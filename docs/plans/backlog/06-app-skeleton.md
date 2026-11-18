# Stage 6: Application Skeleton

## Goal

Build the TUI application event loop, state management, and input dispatch
framework. The TUI operates as a client connected to the server (persistent
or embedded). After this stage, the TUI launches, connects to the server,
displays a placeholder layout, responds to keyboard input, and exits
cleanly on `q`.

## Why

The event loop is the backbone of the TUI. Every feature — rendering the
file list, displaying diffs, handling keybindings — plugs into this loop.
Getting it right early means subsequent stages can focus on building widgets
and features without worrying about the plumbing.

The application state struct is equally important. It holds the file list,
current selection, pane focus, scroll positions, display mode, and
notification buffer. Every UI decision reads from this state, and every
input event mutates it (by sending requests to the server).

## Requirements

1. **Server connection**: on startup, connect to the server (persistent or
   embedded). Send the `init` request with the worktree path and base ref.
   Receive the resolved context.

2. **Terminal setup and teardown**: enter raw mode, enable alternate screen,
   and set up the crossterm backend on startup. Restore the terminal on
   exit (including on panic — use a panic hook or drop guard).

3. **Event loop**: poll for crossterm events (keyboard, resize) and server
   notifications. Dispatch events to the input handler. Re-render after
   each event.

4. **Application state**: a central struct holding at minimum:
   - Connection context (repo, worktree, base_ref, head_ref).
   - The list of `FileEntry` values (received from server).
   - The currently selected file index.
   - Which pane has focus.
   - Content mode (diff / full-file).
   - Render variant (inline/side-by-side or HEAD/base).
   - Scroll position for the diff view.
   - Whether inline comments are visible.
   - Whether the application should quit.

5. **Input dispatch**: a key handler that reads the current pane focus and
   dispatches to the appropriate handler. For now, only `q` (quit) needs
   to work. The dispatch structure should be in place for subsequent
   stages to add bindings.

6. **Placeholder rendering**: render a two-pane layout with placeholder
   content to verify the layout system works. The panes should resize
   with the terminal.

7. **Startup data loading**: on startup, request the file list from the
   server (`list_changed_files`). Populate the application state.

8. **Server notification handling**: the event loop should check for
   server notifications (state changes from other clients) and update
   the application state accordingly.

9. **Clean exit**: `q` quits the application, disconnects from the server,
   restores the terminal, and exits with status 0.

## Acceptance Criteria

- [ ] `cargo run -- <base>` launches a TUI with a two-pane placeholder
      layout (connecting to server or starting embedded).
- [ ] The layout resizes correctly when the terminal is resized.
- [ ] Pressing `q` exits cleanly and restores the terminal.
- [ ] A panic during rendering does not leave the terminal in a broken
      state.
- [ ] The application state is populated with real file data from the
      server on startup.
- [ ] No keyboard input other than `q` is required to work yet, but the
      dispatch structure is in place.
- [ ] Server notifications are received and buffered (even if not yet
      acted on).

## Open Questions

- Should the event loop be fully async (tokio), or use synchronous I/O
  with crossterm's polling? Crossterm's event polling is synchronous,
  but server communication may benefit from async.
- What tick rate should the event loop use? 60fps is typical for TUIs
  but may be overkill if we only re-render on events.
- Should slow server requests (e.g. computing diffs for a large repo)
  show a loading indicator, or block the UI?
