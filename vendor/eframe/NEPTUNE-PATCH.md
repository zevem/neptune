# Neptune's eframe patch

This is eframe 0.36.2 from crates.io (MIT OR Apache-2.0, © Emil Ernerfeldt and
the egui contributors), with one addition:

- `src/native/viewport_windows.rs` and its two `register` calls in
  `src/native/wgpu_integration.rs` record each viewport's native window, and
  `eframe::viewport_window` returns it.

Upstream eframe exposes only the root window. Neptune's Wayland workspace drag
attaches child windows to `xdg_toplevel_drag_v1` and recognises them as drop
targets, which needs each child's `xdg_toplevel` and `wl_surface`.

To update eframe, replace this directory with the new release and reapply the
addition, or drop the patch once upstream exposes viewport windows.
