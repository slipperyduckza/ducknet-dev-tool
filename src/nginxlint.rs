//! Live lint for the site-config editor.
//! Syntax comes from `nginx-lint-parser`; only the parser crate ships, so the cli/wasmtime stack stays out.
//! Semantic checks assume partial configs (`server { ... }` included from http); empty means clean.
//! Upstream: https://github.com/walf443/nginx-lint (MIT).

use nginx_lint_parser::ast::{Block, Config};
use nginx_lint_parser::line_index::LineIndex;
use nginx_lint_parser::syntax_kind::{SyntaxElement, SyntaxKind, SyntaxNode};
use nginx_lint_parser::{parse_string_rowan, parse_string_with_errors};

/// Cap on diagnostics; keeps the QML label readable.
const MAX_ISSUES: usize = 30;

/// One lint diagnostic.
fn issue(line: usize, col: usize, error: bool, msg: &str) -> String {
    format!(
        "L{}:{} [{}] {}",
        line,
        col,
        if error { "error" } else { "warn" },
        msg
    )
}

/// Lint an editor draft; empty means clean.
pub fn lint_site_draft(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let index = LineIndex::new(text);
    let (config, syntax_errors) = parse_string_with_errors(text);

    // Layer 1 — syntax errors from the rowan parser (authoritative).
    for err in &syntax_errors {
        let pos = index.position(err.offset.min(text.len()));
        out.push(issue(pos.line, pos.column, true, &err.message));
        if out.len() >= MAX_ISSUES {
            return out.join("\n");
        }
    }

    // Missing semicolons: the parser treats newlines as whitespace, so walk the CST.
    check_missing_semicolons(text, &index, &mut out);
    if out.len() >= MAX_ISSUES {
        out.truncate(MAX_ISSUES);
        return out.join("\n");
    }

    // Layer 2 — semantic checks over the recovered AST.
    check_config(&config, &mut out);

    if out.len() > MAX_ISSUES {
        out.truncate(MAX_ISSUES);
    }
    out.join("\n")
}

/// Top-level checks: site files should be `server { ... }` blocks.
fn check_config(config: &Config, out: &mut Vec<String>) {
    let mut saw_server = false;
    for dir in config.directives() {
        if dir.is("server") {
            saw_server = true;
            if let Some(block) = &dir.block {
                check_server_block(dir, block, out);
            } else {
                let s = dir.span.start;
                out.push(issue(
                    s.line,
                    s.column,
                    true,
                    "server directive needs a { ... } block",
                ));
            }
        } else if dir.is("location") {
            let s = dir.span.start;
            out.push(issue(
                s.line,
                s.column,
                true,
                "location must live inside a server block, not at top level",
            ));
        }
    }
    // Blank drafts stay clean; the editor may be empty while typing.
    if !saw_server && config.directives().next().is_some() && out.is_empty() {
        let first = config.directives().next().unwrap();
        let s = first.span.start;
        out.push(issue(
            s.line,
            s.column,
            false,
            "no server block found — site files normally define server { ... }",
        ));
    }
}

fn check_server_block(server: &nginx_lint_parser::ast::Directive, block: &Block, out: &mut Vec<String>) {
    let mut listens: Vec<(String, usize, usize)> = Vec::new();
    let mut has_server_name = false;
    for dir in block.directives() {
        if dir.is("listen") {
            let arg = dir.first_arg().unwrap_or("").to_string();
            let s = dir.span.start;
            if listens.iter().any(|(a, _, _)| *a == arg) {
                out.push(issue(
                    s.line,
                    s.column,
                    false,
                    &format!("duplicate listen {arg} in this server block"),
                ));
            } else {
                listens.push((arg, s.line, s.column));
            }
        } else if dir.is("server_name") {
            has_server_name = true;
        } else if dir.is("location") {
            if let Some(lblock) = &dir.block {
                check_location_block(dir, lblock, out);
            }
        }
    }
    if listens.is_empty() {
        let s = server.span.start;
        out.push(issue(
            s.line,
            s.column,
            false,
            "server block has no listen directive (defaults to *:80)",
        ));
    }
    if !has_server_name {
        let s = server.span.start;
        out.push(issue(
            s.line,
            s.column,
            false,
            "server block has no server_name (will catch every request on this port)",
        ));
    }
}

/// `root` inside `location` is fragile; prefer `root` on the server or `alias` here.
fn check_location_block(
    _location: &nginx_lint_parser::ast::Directive,
    block: &Block,
    out: &mut Vec<String>,
) {
    for dir in block.directives() {
        if dir.is("root") {
            let s = dir.span.start;
            out.push(issue(
                s.line,
                s.column,
                false,
                "root inside location is fragile — prefer root on the server, or alias here",
            ));
        } else if dir.is("location") {
            if let Some(nested) = &dir.block {
                check_location_block(dir, nested, out);
            }
        }
    }
}

/// CST walk for missing semicolons.
fn check_missing_semicolons(text: &str, index: &LineIndex, out: &mut Vec<String>) {
    let (root, _) = parse_string_rowan(text);
    let len = text.len();
    walk_cst_node(&root, index, len, out);
}

fn walk_cst_node(node: &SyntaxNode, index: &LineIndex, len: usize, out: &mut Vec<String>) {
    for child in node.children_with_tokens() {
        if let SyntaxElement::Node(child_node) = child {
            match child_node.kind() {
                SyntaxKind::DIRECTIVE => check_cst_directive(&child_node, index, len, out),
                SyntaxKind::BLOCK => {
                    if !is_raw_cst_block(&child_node) {
                        walk_cst_node(&child_node, index, len, out);
                    }
                }
                _ => walk_cst_node(&child_node, index, len, out),
            }
        }
        if out.len() >= MAX_ISSUES {
            return;
        }
    }
}

/// `*_by_lua_block` holds raw Lua; never lint inside.
fn is_raw_cst_block(block: &SyntaxNode) -> bool {
    if let Some(parent) = block.parent() {
        if parent.kind() == SyntaxKind::DIRECTIVE {
            for child in parent.children_with_tokens() {
                let kind = child.kind();
                if kind.is_trivia() {
                    continue;
                }
                if kind == SyntaxKind::IDENT {
                    if let Some(token) = child.into_token() {
                        return token.text().ends_with("_by_lua_block");
                    }
                }
                break;
            }
        }
    }
    false
}

fn check_cst_directive(directive: &SyntaxNode, index: &LineIndex, len: usize, out: &mut Vec<String>) {
    let has_semicolon = directive
        .children_with_tokens()
        .any(|c| c.kind() == SyntaxKind::SEMICOLON);
    let has_block = directive
        .children()
        .any(|c| c.kind() == SyntaxKind::BLOCK);

    check_merged_cst_directives(directive, index, len, out);

    for child in directive.children() {
        if child.kind() == SyntaxKind::BLOCK && !is_raw_cst_block(&child) {
            walk_cst_node(&child, index, len, out);
        }
    }

    // Directive with no terminator at all (EOF or before `}`).
    if !has_semicolon && !has_block {
        if let Some(last) = directive
            .children_with_tokens()
            .filter(|c| !c.kind().is_trivia())
            .last()
        {
            let end_offset: usize = last.text_range().end().into();
            let pos = index.position(end_offset.saturating_sub(1).min(len));
            out.push(issue(
                pos.line,
                pos.column,
                true,
                "missing semicolon at end of directive",
            ));
        }
    }
}

/// Detect directives merged by a missing `;` on adjacent lines.
fn check_merged_cst_directives(
    directive: &SyntaxNode,
    index: &LineIndex,
    len: usize,
    out: &mut Vec<String>,
) {
    let children: Vec<SyntaxElement> = directive.children_with_tokens().collect();
    let mut seen_name = false;
    let mut seen_args = false;

    for (i, child) in children.iter().enumerate() {
        let kind = child.kind();
        if !seen_name {
            if is_value_kind(kind) {
                seen_name = true;
            }
            continue;
        }
        if is_value_kind(kind) {
            seen_args = true;
            continue;
        }
        if kind == SyntaxKind::NEWLINE && seen_args {
            let next_idx = children[i + 1..]
                .iter()
                .position(|c| !c.kind().is_trivia())
                .map(|offset| i + 1 + offset);
            if let Some(next_idx) = next_idx {
                if children[next_idx].kind() == SyntaxKind::IDENT
                    && starts_own_cst_directive(&children, next_idx)
                {
                    if let Some(last) = children[..i]
                        .iter()
                        .rev()
                        .find(|c| !c.kind().is_trivia())
                    {
                        let end_offset: usize = last.text_range().end().into();
                        let pos = index.position(end_offset.saturating_sub(1).min(len));
                        out.push(issue(
                            pos.line,
                            pos.column,
                            true,
                            "missing semicolon at end of directive",
                        ));
                    }
                    seen_args = false;
                }
            }
        }
        if out.len() >= MAX_ISSUES {
            return;
        }
    }
}

fn starts_own_cst_directive(children: &[SyntaxElement], name_idx: usize) -> bool {
    children[name_idx + 1..]
        .iter()
        .take_while(|c| c.kind() != SyntaxKind::NEWLINE)
        .any(|c| is_value_kind(c.kind()) || c.kind() == SyntaxKind::BLOCK)
}

fn is_value_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::IDENT
            | SyntaxKind::ARGUMENT
            | SyntaxKind::VARIABLE
            | SyntaxKind::DOUBLE_QUOTED_STRING
            | SyntaxKind::SINGLE_QUOTED_STRING
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "server {\n    listen 80;\n    server_name example.test;\n    root /home/u/WebRoots/example.test;\n\n    location / {\n        try_files $uri $uri/ =404;\n    }\n}\n";

    #[test]
    fn good_config_is_clean() {
        assert_eq!(lint_site_draft(GOOD), "");
    }

    #[test]
    fn missing_semicolon_reports_line() {
        let bad = "server {\n    listen 80\n    server_name example.test;\n}\n";
        let out = lint_site_draft(bad);
        assert!(out.contains("L2:"), "expected L2 issue, got:\n{out}");
        assert!(out.contains("[error]"), "got:\n{out}");
    }

    #[test]
    fn unclosed_brace_is_error() {
        let bad = "server {\n    listen 80;\n";
        let out = lint_site_draft(bad);
        assert!(out.contains("[error]"), "got:\n{out}");
    }

    #[test]
    fn missing_listen_and_server_name_warn() {
        let bad = "server {\n    root /srv/x;\n}\n";
        let out = lint_site_draft(bad);
        assert!(out.contains("no listen"), "got:\n{out}");
        assert!(out.contains("no server_name"), "got:\n{out}");
        assert!(!out.contains("[error]"), "got:\n{out}");
    }

    #[test]
    fn duplicate_listen_warns() {
        let bad = "server {\n    listen 80;\n    listen 80;\n    server_name x.test;\n}\n";
        let out = lint_site_draft(bad);
        assert!(out.contains("duplicate listen 80"), "got:\n{out}");
    }

    #[test]
    fn top_level_location_is_error() {
        let bad = "location / {\n    try_files $uri =404;\n}\n";
        let out = lint_site_draft(bad);
        assert!(out.contains("[error]"), "got:\n{out}");
        assert!(out.contains("inside a server block"), "got:\n{out}");
    }

    #[test]
    fn root_in_location_warns() {
        let bad = "server {\n    listen 80;\n    server_name x.test;\n    location / {\n        root /srv/x;\n    }\n}\n";
        let out = lint_site_draft(bad);
        assert!(out.contains("root inside location"), "got:\n{out}");
    }

    #[test]
    fn empty_draft_is_clean() {
        assert_eq!(lint_site_draft(""), "");
        assert_eq!(lint_site_draft("# just a comment\n"), "");
    }

    #[test]
    fn multiline_directive_args_not_flagged() {
        // Continued arguments on following lines are legal; never flag them.
        let cfg = "server {\n    listen 80;\n    server_name x.test;\n    set $a foo\n        bar;\n}\n";
        assert_eq!(lint_site_draft(cfg), "");
    }
}
