# Design

## Decisions

- **`gtk::DropTargetAsync` (not `DropTarget`) accepting copy|move, finished as copy.**
  File managers offer copy|move and Hyprland selects move; a copy-only `DropTarget` rejects
  that, and a copy|move one finishes as a move (which may make the source delete the
  original). Reading the value ourselves lets us `finish(COPY)`. If GTK's preferred
  document-portal transfer can't be read, fall back to `text/uri-list`.
- **The drop target sits on the compose window, in the capture phase.**
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

- Drops are tested with a real file manager in the test display via a virtual pointer
  (`dev/test-display.sh pointer`); the first version passed GTK-only reasoning but failed
  with Dolphin because of the move/copy negotiation above.
