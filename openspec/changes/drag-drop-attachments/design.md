# Design

## Decisions

- **One `gtk::DropTarget` for `gdk::FileList` on the compose window, in the capture phase.**
  The body `TextView` has its own drop target for text; file managers also offer
  `text/plain`, so a bubble-phase target on the window would lose to the text view and the
  file path would be typed into the body. In the capture phase the window sees the drop
  first.
- **Native files are attached, other URIs become text.** `FileList` also carries `http:`
  links dragged from a browser; GIO would happily download them via GVfs, which isn't what
  someone dragging a link expects. `gio::File::is_native()` separates the two.
- **Async loading** with `File::load_contents_future` on the main context
  (`glib::spawn_future_local`): no UI freeze on a 50 MB file, works for files on GVfs mounts
  too, and replaces the picker's blocking `std::fs::read`. Directories are detected with
  `query_file_type` and skipped with a toast.
- **Size limit is advisory**: a toast at > 25 MB total, not a block; servers differ.
- **Hover feedback** via `enter`/`leave` on the drop target toggling a `drop-hover` CSS
  class on the window content (dashed `@accent_color` outline) and revealing a small
  "Drop to attach" label in the header area.

## Risks / Trade-offs

- Drag and drop can't be driven by the automated test display (no pointer injection), so
  the drop itself is verified by hand; the loading/labeling logic is shared with the picker
  and the pure helpers are unit-tested.
