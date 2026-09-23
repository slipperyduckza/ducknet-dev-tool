#pragma once
#include "rust/cxx.h"

namespace ducknet {

// Set the application window icon from an image file (svg/png).
// An empty path falls back to QIcon::fromTheme("org.kde.ducknetdevtool").
// Signature must match the cxx::bridge declaration (rust::Str).
void set_window_icon_from_path(rust::Str path);

} // namespace ducknet
