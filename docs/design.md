# Terminal interface direction

Neptune's interface was rebuilt on 2026-09-30 around a native, Apple-style visual
language: one quiet window material, content that sits in it as rounded
surfaces, depth from elevation rather than borders, and a single accent reserved
for focus and selection. The terminal remains the dominant surface. Chrome earns
its space by being useful at a glance and then getting out of the way.

This replaces the earlier StarkIDE-derived interface. That design, its tokens and
its acceptance record remain in git history and the [evidence archive](artifact-archive.md);
they are historical evidence, not the current direction.

## Principles

- **Content first.** A single terminal has no header of its own; the toolbar
  names it. Pane headers, focus rings and dimming appear only when there is
  more than one terminal to tell apart.
- **Nothing covers or reflows the shell.** Search lives in the toolbar. Opening
  it, the command palette or a sheet never resizes a PTY. Status that belongs to
  a pane floats inside that pane; application messages sit in the window's top
  trailing corner, clear of the prompt.
- **The terminal owns its keys.** The focused terminal holds keyboard focus, so
  Tab, arrows and Escape reach the shell. Outside sheets and menus the toolkit
  never moves focus with Tab or the arrow keys; chrome is reached by pointer,
  shortcut, the command palette or assistive technology. Escape cancels a
  terminal drag, or leaves a sheet or a focused search field; otherwise it goes
  to the shell. A message never
  takes a key the shell is waiting for: it is dismissed by its button or the
  "Dismiss message" command, and by Escape only when no terminal is open. A key
  that closes a menu is not also sent to the shell, and Enter confirms a dialog
  only as a fresh press after the dialog is visible.
- **Quiet until needed.** Controls for a pane fade in on hover or focus. Motion
  is short (120–160 ms, ease-out) and used for state changes the pointer
  caused; keyboard-opened surfaces such as the command palette appear at once.
  A settled window does not repaint.
- **Every control is reachable and named.** Custom controls report native roles
  and stable labels; sheets are exposed as windows and keep focus inside them.

## Materials and colour

Tokens live in `src/theme.rs` (`Palette`, `metrics`). Use them rather than
literal colours.

| Token | Graphite | Dusk | Light | Use |
| --- | --- | --- | --- | --- |
| `chrome` | `#1c1c1f` | `#1e1c2b` | `#ececef` | Window material: toolbar and sidebar |
| `bg` | `#101012` | `#12111c` | `#ffffff` | Terminal content surface |
| `elevated` | `#29292d` | `#2b2840` | `#ffffff` | Sheets, menus, floating status |
| `fg` | `#ececf1` | `#e9e7f5` | `#1d1d1f` | Primary text |
| `secondary` | `#a0a0a8` | `#a29fb8` | `#5e5e66` | Supporting text, resting icons |
| `muted` | `#6d6d76` | `#6f6c87` | `#8e8e95` | Section labels, hints |

`control`, `hover`, `pressed`, `separator` and `border` are translucent white
(dark themes) or black (Light) so they read correctly on any material. Floating
surfaces carry a soft shadow and a one-pixel `border`; regions that share a
material are divided by a `separator` hairline or by space alone.

One theme determines the palette for the entire window and every terminal.
Graphite, Dusk and Light retain their original materials. The 712 imported
palettes and saved custom themes supply exact terminal colors; window materials
are derived from their background, with interface text contrast of at least
4.5:1 and focus accents of at least 3:1 against the main surfaces. The accent
comes from ANSI blue and marks focused panes, selected rows, primary buttons,
switches, focus rings and search matches. `on_accent` keeps button labels
readable. The older `accent` config field remains compatible with the three
original themes; there is no separate accent control in preferences. Cursor and
selection colors come from the chosen palette. Red marks destructive actions
and stopped terminals; a destructive control's label is white unless an
imported red is too light to carry it. Amber (`attention`) is reserved for what
waits for the person: unread terminal alerts (the pane ring, the bell's dot and
unread counts) and agents waiting for input (their row's mark and state, their
count on the Agents tab, the dot on the panel toggle, and the count of
unresolved review comments beside a linked pull request) and what a project
needs the person for (its "Needs you" rows and counts). In the three
original themes it turns yellow when the accent itself is orange, so an alert
never reads as focus; an imported or custom palette supplies its ANSI yellow,
held to 3:1 against the main surfaces.

A linked pull request's number says where it stands: the accent while open,
`secondary` as a draft, `muted` once closed and purple (`merged`, the palette's
ANSI magenta held to 3:1) once merged, which also changes its icon. Its checks
are a green check, a red cross or a yellow ring, so no state rests on colour
alone, and the tooltip says each in words.

Notification rows mark their workspace with a tile in a stable identity colour
taken from the workspace id. The identity colour is decoration only; state is
always also carried by text, a badge or position.

## Geometry

Radii are concentric: the window corner (16) equals the pane corner (10) plus
the gutter between them (6). Sheets use 14, controls and rows 8, menus 10.
Restored windows use the same transparent outer corners and hairline on Linux
and macOS. Windows draws its own corners and border, so the window paints square
corners there. Maximized windows keep rounded corners on macOS and use square
corners on Linux. Fullscreen windows use square corners on every platform;
restoring the window brings the rounded corners back.
The toolbar is 44 points tall and pane headers 34. Controls are 30 points high;
icon buttons keep a 28-point target around a 16-point glyph.

## Layout

- **Sidebar.** Full window height, like a native source list, and resizable from
  its trailing edge (saved on release; double-click restores the default). It
  hosts the window controls, the workspace list and a footer with "New
  workspace" and Preferences. Each row shows the name (led by a red dot once
  every terminal in it has stopped), a
  path that keeps its final directory, and either the pane count or, on hover,
  a "more" button with the same menu as a secondary click. A workspace
  folder inside a Git repository is followed by a branch icon and the
  branch's name in the same muted text, then a 5-point dot in the palette's
  yellow while the repository has uncommitted changes; the branch may take up
  to six tenths of the line and the path gives way to it. A workspace
  connected over SSH shows a globe and its host in place of the path, and its
  menu offers "Disconnect from SSH" where a local one offers "Connect over
  SSH…". Double-click renames.
  Secondary-click empty list space for "New workspace", "New SSH
  workspace", and "New workspace group". Ungrouped workspaces and folder groups
  share an ordered list. A folder row shows a disclosure chevron, an outline
  folder and its name; its count yields to a plus control on hover or focus.
  Clicking the row reveals or hides indented workspaces with the native 120 ms
  ease-out collapse animation and a matching fade. Double-click renames; the
  folder menu creates local or SSH workspaces, sets a default local directory,
  renames, or removes the folder while retaining its workspaces. "Default
  directory…" opens a compact sheet with the group name, editable path, native
  "Browse…" folder picker, Save, Cancel and "Use home directory". Selecting a
  folder edits the path until Save; the sheet omits explanatory paragraphs.
  Tabs and splits inherit their source terminal’s directory. Workspace menus
  offer "Move to group" and
  "Ungrouped". Collapse never suspends output. Group organization and collapsed
  state restore, and shortcut/palette selection reveals the selected workspace.
  Dragging a folder row reorders groups as blocks with their visible workspaces,
  using the same lift, neighbor movement, edge scrolling and cancellation as
  workspace rows. Groups can sit at the top or between ungrouped workspaces;
  ungrouped rows can move around groups. The mixed order restores and determines
  workspace navigation and shortcut hints, including children of collapsed groups.
  Dragging a workspace row reorders the workspaces within its group: the row lifts onto the elevated
  material and follows the pointer, its neighbours ease aside to show where it
  will land, and it settles into that gap on release. Holding it at the list's
  edge scrolls a long list, Escape puts it back, and nothing is saved before
  release. "Move up" and "Move down" in the row menu and the command palette
  reorder without a pointer and take effect at once.
  While a terminal is carried, every other workspace's row on the same machine
  is a drop destination and takes the accent under the pointer.
  The sidebar yields to terminal content below 820 points of window width.
  Toggling it slides it in or out over 160 ms, from the button, the shortcut
  or the command palette alike. The window controls stay in place, the toggle
  travels between the sidebar's trailing edge and its place in the toolbar, and
  the terminals follow the sidebar's edge. Each shell is resized once, to the
  size it will rest at, and a toggle reversed midway turns around from where it
  is. Restored state and the width threshold take effect at once.
- **Toolbar.** Belongs to the content area and drags the window. Leading: the
  workspace name and the focused terminal's program and directory. Centre: a
  command field that opens the palette, replaced by the search field while
  searching. Trailing: find and split controls for the focused terminal, the
  notification bell and the right panel toggle, which stays pressed while its
  panel is open and carries a dot in the attention colour while an agent waits
  for input, or the project of the workspace in view needs the person, where
  the panel does not show it. When
  the sidebar is hidden the toolbar also carries the window controls, the
  sidebar toggle and "New workspace". In narrow windows the command field
  collapses to an icon and search takes the title's room.
- **Right panel.** A panel at the trailing edge, below the toolbar and in the
  window material. Toggling it slides it in from the trailing edge over 160
  ms with a matching fade, from the button, the shortcut or the command
  palette alike. As with the sidebar, the terminals follow its edge, each shell
  is resized once to the size it will rest at, and a toggle reversed midway
  turns around from where it is. Its leading edge resizes it (220–560 points;
  double-click restores 300) and it always leaves the terminals 240 points. A
  34-point strip at its top holds five tabs, "Files", "Agents", "Changes",
  "Project" and "Pull request", of equal width unless the panel is too narrow
  for a name, where they are set closer and each takes what its name needs;
  near its narrowest the last is named "PR". They are drawn like terminal tabs: the one in view takes a faint fill. They
  are chosen with the pointer or the command palette, never with Tab or the
  arrow keys. The Agents tab counts the agents waiting for input in a pill in
  the attention colour, and the Project tab counts what its project needs the
  person for in the same way. The panel opens on the tab last shown.
- **File explorer.** The Files tab shows the focused terminal's folder; it
  follows that terminal as it changes directory, and says why when the
  workspace is connected over SSH. The
  header names the folder and carries "New file", "New folder" and "Refresh".
  Under it are the search field and, while searching or when its "…" control is
  on, the "files to exclude" field. The tree lists every item, hidden ones
  included, folders first; rows are 26 points, indent 14 a level, and show a
  disclosure chevron for folders. A click opens a folder or previews a file;
  the selected row takes the accent tint. A secondary click opens the item's
  menu: new file or folder inside a folder, open with the default application,
  reveal in the platform's file manager, copy path, copy relative path, rename
  and delete. A name is typed in the tree itself, where the item is or will
  be: Enter uses it, Escape abandons it, and a name that cannot be used
  outlines the field in red with the reason on the next row. Deleting is
  permanent, so it waits for a sheet that names the item. The preview is a
  content surface at the bottom of the panel, resized by the divider above it:
  text in the terminal face with line numbers, pictures fitted to its room,
  and a plain statement for anything else. All reading, searching and changing
  of files happens off the UI thread; the folders in view are read again every
  two seconds, and a frame is drawn only when something changed. Nothing is
  read while the Agents tab is the one in view.
- **Agents.** The Agents tab lists every CLI agent running in a terminal of any
  workspace, under the headings "Needs input", "Working" and "Idle", in that
  order, each with its count. A row is 46 points: a mark, what the agent calls
  its conversation (or the agent's name), and how long it has been in its
  state; beneath, the state in words, the agent and its workspace. The mark is
  filled in the attention colour while the agent waits for a person, a green
  dot in a halo while it works and a muted ring at rest; the state is always
  also said in words ("Needs permission", "Asked a question", "Plan needs
  approval", "Needs input", "Working", "Idle"). The row of the focused terminal
  takes a faint fill. Clicking a row reveals and focuses its terminal. An
  empty list says how an agent comes to be listed. Rows change when an agent
  reports or its title changes; nothing animates, and a list in view is drawn
  again every 30 seconds only so that its ages stay true.
- **Changes.** The Changes tab shows what Git says about the focused
  terminal's folder. The 30-point header carries a branch icon, the branch's
  name and, in muted text, the commits to push and to pull beside up and down
  arrows; "Refresh changes" sits at its trailing edge. Under it a segmented
  control chooses "Working tree" or "Branch", and a line counts the files
  ("3 files changed since origin/main") with the lines added in green and
  removed in red at its trailing edge. Rows are 26 points: a status letter in
  its colour (M yellow, A and U green, D and ! red, R accent), the file's name,
  its folder in muted text, and its own added and removed counts. The selected
  row takes the accent tint and its diff opens in a content surface at the
  bottom of the panel, resized by the divider above it: the terminal face,
  one line number, a sign, and a faint green or red fill across added and
  removed lines; hunk headings are muted on a faint fill. Colour never stands
  alone: every line carries its sign and every row its letter. A folder that
  is not a repository, an SSH workspace, a missing Git and a branch with
  nothing to compare with each say so in muted text. Git runs off the UI
  thread; a frame is drawn only when what it reported changed.
- **Pull request.** The Pull request tab shows one pull request, opened by
  pressing its number on a terminal's tab; pressed as a terminal's link is
  (Ctrl or Command), the number opens the browser instead. A 30-point row
  leads with a back chevron, the state's icon, the repository in the
  secondary colour and the number in the state's colour, which together open
  the host's page, and trails "Refresh pull request" and a "⋯" menu. Under
  it the title is set in the medium weight at 14 points on two lines at
  most, then a pill in the state's tint with its icon and word beside who
  opened it and when it last changed, then a segmented control "Summary",
  "Timeline", "Code". The state keeps the colours of a linked number:
  accent open, `secondary` draft, `muted` closed, `merged` once merged.
  Summary scrolls and leads with one card on `control` that says where the
  pull request stands in a line (a green check "Ready to merge", a red
  warning for conflicts, requested changes or failing checks, a yellow ring
  while checks run, a muted ring otherwise) with its one action at the
  trailing edge, under the line where both do not fit: the accent merge
  button, "Ready for review" or a plain "Reopen". Merging and closing ask
  once more in the card, with "Cancel" and the confirming button, which is
  red for closing. What the host refused is a card in the red tint with its
  words and a dismiss control. Then the branches in the terminal face (the
  source is a chip that copies its name and says "Copied"), the lines added
  and removed, and labelled rows that wrap: reviewers and assignees as a
  disc in an identity colour with an initial (no picture is loaded) and the
  reviewer's verdict as a check, a cross, a comment or a ring, labels as
  pills. "Description" and "Checks" are headings that fold; a check is a
  26-point row with the mark of a linked number's checks, its name, its
  workflow in muted text and how long it ran or its state in words.
  Comments are cards on `control`: the writer's disc and name, what they did
  ("commented", "approved" in green, "requested changes" in red), the age
  and an arrow to the host, the file and line of a review conversation in
  the terminal face with "Outdated" and "Resolved", the Markdown, and
  replies set in behind a two-point bar. Timeline is the same things said
  in order on a rail of 19-point marks joined by a `separator` hairline.
  Code is the Changes tab's list and diff. A round control on the elevated
  material floats over the bottom trailing corner and opens the field for a
  comment or review at the bottom of the tab, under a hairline, with its
  one accent button; a field that leaves view gives up the keyboard and
  keeps its text. The pull requests opened in the tab are pills on a 28-point
  strip above all this, in the terminal tabs' language, each with a cross.
  Rows that can be added to end in a "+" that opens a menu with what is in
  use ticked. Under a comment its reactions are pills of a word and a count
  (the person's own in the accent tint) with a "+" for another, and a review
  conversation trails "Reply" and "Resolve"; a reply and an edited
  description are written in a field in place, with "Cancel" and one
  accent button. In Code a line something is said of carries a two-point
  accent bar at its leading edge, and the lines of the open diff take a
  press that writes a comment on them. Without a pull request the tab lists the linked ones as
  46-point rows of an icon, `owner/repo#N` and the state in words. Reading
  and every change run off the UI thread; a pull request in view is drawn
  again every 30 seconds for its ages, and otherwise only when what was
  read changed.
- **Project.** The Project tab holds the project of the workspace in view,
  and is a conversation with its lead. A workspace without one shows the
  creation form: "New project", a muted line naming the directory, and one
  multi-line field with the send control inside it, which Enter also
  presses. The line under the field names the lead ("Claude Code ›") and
  unfolds its options: the CLI (a two-part segmented control when both are
  installed), its model and its effort, each a select whose first choice is
  "Default". A muted paragraph says what a lead is; a project that worked in
  the directory before is offered in a card instead. The form scrolls where
  the panel is short. A workspace over SSH, no workspace and an unsupported
  system each say so in muted text. A project shows, from the top: a title
  row with its name, its directory in muted text, a "Project details" control
  and a "⋯" menu; what needs the person; a strip of its agents; the chat;
  the composer; and a line naming the lead. Nothing else is drawn until it is
  true or asked for. What needs the person is a row in the attention tint
  with an amber mark, a title, at most one accent action at its trailing
  edge ("Open", "Try again", "Resume") and its explanation in the secondary
  colour: amber is the mark, never a paragraph. A project the person paused
  shows a plain row "Paused" with "Resume" there instead; pausing itself is
  in the "⋯" menu. The strip of the agents is one 26-point row, absent
  without agents, that never grows: the chat's height does not depend on
  how many agents there are or on whether they are being looked at. It
  holds a pill for each agent in the terminal tabs' language (22 points
  tall, radius 7, the label in the medium weight at 11.5): the agent's mark,
  its number and its title, in the secondary colour and unfilled at rest,
  with the tab's hover fill; an agent whose terminal is a tab has the fill
  and the foreground colour of the tab in view; one that waits for a person
  has the attention tint of the rows above. Agents that wait come first,
  then those at work, then the rest, each by number. Pills are as wide as
  their words up to 130 points with 4 points between them; where they do
  not fit they narrow alike, down to the mark and the number, and past that
  the strip scrolls sideways by wheel, trackpad or drag with no bar, its
  clipped edge fading into the surface. A pill is pressed as a tab is and
  opens the agent's terminal; resting on it shows its number and title, its
  CLI and branch, and its state with its age. At the strip's trailing edge,
  never scrolling, a control with the agents icon, their count and a
  chevron opens the list of all of them in a popover anchored under the
  strip and as wide, over the chat: a line that says how they stand
  ("2 working · 1 ready for review"), then a row for each in the pills'
  order, at most three fifths of the tab's height and scrolling past that.
  The control's tooltip and accessible name say the same words. The
  popover closes on Escape, a press outside, a second press on its control
  and a press on a row. An agent's row there is two lines: a mark, its
  number and title and its state in words, then its CLI, branch, pull
  request chips and age. The mark and the state follow the Agents tab's
  vocabulary. A secondary click on an agent's pill, and on its row on the
  Agents tab, opens a menu with "Open terminal" and, once the terminal is a
  tab that can go out of view again, "Send to background"; a terminal's own
  menu and the command palette offer the same only where it applies, never
  disabled. What needs the person takes at most two fifths of the room over
  the composer less the strip's row, leaves the chat about four lines and
  scrolls by itself with the bar in view; the strip never scrolls away. The
  chat has no speaker labels: the person's messages
  are filled bubbles at the trailing edge, the lead's replies plain text
  with its Markdown drawn natively and a copy control at the end of each;
  what Neptune did is a muted one-line row, and what it heard is an
  outlined card with its actions beneath ("Open terminal", "Changes", or
  "Allow", "Decline" and "Watches" on a proposed watch). A turn the lead
  finished shows its reply alone, the last block it wrote: what it said
  and what Neptune did before that is its work, folded behind one row
  "Worked for 4m 52s" with a chevron after the words, in the muted ink and
  size of the rows of what Neptune did, over a `separator` hairline. The
  whole row is pressed and unfolds the work in place and in order, with
  the chevron turned down; pointer and focus draw it in the text's ink.
  The row is absent where nothing would be folded. A turn that runs, was
  stopped or failed is shown in full, and a refused request, a card and
  a notice are never folded. Markdown keeps a
  side panel's scale: the first two heading levels are a little larger
  than the text (16 and 14.5 points, medium) and the others are told apart
  by weight and ink; lists nest by 18 points to four levels, tasks carry a
  box, a quote has a two-point bar in `border`, a table has a medium
  header over a `border` line and `separator` hairlines between rows, and
  code sits on `control` with long lines wrapped. A table's cells wrap
  before the table scrolls sideways inside its own width; nothing makes
  the panel scroll sideways. Emphasis is the regular face slanted, since
  no italic is bundled. A link is the text's ink underlined, never the
  accent; it shows where it leads under the pointer and opens only when
  pressed, and only a web address or a file of this computer does. A
  picture is never loaded: it is the link its words are. A card is an
  event, not a message: its headline, at most two lines of the report's
  words without their marks, ended after a whole word, and its actions. A
  report that two lines do not hold has the chevron of the rows above it
  before its headline, which unfolds all of it in place, drawn as a reply
  is, and folds it again. What the person is asked to allow is always
  shown whole. The composer is fixed at the bottom with one control that sends,
  or stops the lead while it works and the field is empty, and a paperclip
  beside it that attaches files. Attached files are outlined chips of an
  icon and a name cut to fit: over the field, each with a control that
  takes it off, at most three rows before they scroll, and under the
  person's message in the chat. Files held over the chat tint it in the
  accent with "Drop to attach for the lead", as a terminal does. The line under
  it names the lead's CLI, model and effort and opens the settings; at its
  trailing edge it says how long the lead has worked and how many messages
  wait. No field takes the keyboard unasked, and Escape returns it to the
  terminal with the draft kept. "Project details" replaces the chat with a
  row of a back chevron and a segmented control "Context", "Watches",
  "Settings", and stays pressed meanwhile; where something needs the person
  one amber line counts it and leads back to the chat. Context is an editor
  for the person's instructions, whose "Save" and "Revert" appear only while
  it differs from what is kept, over the kept files, longest untouched
  first. Watches lists one row per watch with a "⋯" menu (Pause or Resume,
  Run now, Delete…), a proposed one with "Allow" and "Decline" in its row,
  and "Add watch" at the trailing edge of its label; the form opens over the
  list and chooses how often in words, with "Custom…" for a number of
  minutes. A row says how soon it runs and how long ago it ran, and the
  list in view is drawn again every 30 seconds while a watch has such a
  time. Settings is grouped cards of setting rows as in Preferences: the
  lead (its CLI, said and not chosen, its model and effort), the agents the
  lead starts for each CLI (model, effort and, for Claude Code, ultracode),
  and the project (its name and directory). Every select names its value
  ("Default" for the lead, "Lead decides" for agents); "Custom…" puts a
  field in the select's place for a model no list names, outlined in the
  problem colour while a CLI would not take it. A change applies at once. An
  action that cannot be taken is a plain control, not a fainter accent.
  Deleting a file or a watch asks once more inside its row; removing the
  project waits for a sheet that says what is deleted and what stays. Where
  the panel has no room, the same body opens in a centred sheet whose title
  row is the project's header; in a sheet too low for whole rows, what needs
  the person is one line a row. Reading and writing of project files and
  all talk with the lead happen off the UI thread; while a turn runs and the
  tab is in view it is drawn again once a second for the elapsed time, and
  otherwise only when something changed.
- **Window controls.** Close, minimize and maximize are three lights at the
  leading edge. Glyphs appear as the pointer approaches, and the lights turn
  neutral in an inactive window. Their accessible names are "Close window",
  "Minimize" and "Maximize".
- **Panes.** Rounded surfaces in the chrome, separated by a 6-point gutter that
  is also the split handle (a grip appears on hover; double-click evens the
  whole row or column it divides). A new split takes an equal share of its
  row or column and a closed one returns its share, the others keeping their
  proportions, so equal terminals stay equal. The surface takes the terminal's resolved background, so a program
  that changes it stays seamless. Each place in the layout holds one or more
  terminals as tabs, with one in view. With several terminals, each place has
  a header with a tab per terminal, naming the program and directory (the
  host, for a remote terminal), followed by "New tab", zoom and split
  controls. A tab whose terminal runs Claude Code, Codex, OpenCode or pi leads
  with that agent's mark at 13 points while it has room, and the chip of a
  carried tab shows the same mark. The tab in view is filled once it has neighbours; a tab out of
  view shows a dot for unread alerts, and each tab closes from its own button
  or a middle click. The focused pane carries an accent ring and the others
  recede slightly. Zooming shows one place, with its tabs, and a "Zoomed" chip
  in the toolbar that restores the layout.
- **Moving a terminal.** A terminal's tab is its handle. Dragging it lifts
  the terminal: its pane recedes, a chip with its name follows the pointer, and
  the place under the pointer shows in the accent the area a drop would take.
  The nearest edge gives the terminal a place of its own beside that one,
  which is also how a tab leaves the place it shares; the centre adds it to
  that place's tabs, and a drop on the header puts it between the tabs under
  the pointer, marked by an insertion line. The preview glides between areas
  and fades where it is released. Dropping on another workspace's sidebar row
  moves the terminal there as the last tab beside that workspace's focused
  terminal; the view stays where it was; moving a workspace's last terminal
  removes the workspace and follows the terminal. The shell keeps running throughout. A drop where
  the terminal already is changes nothing, and Escape or an opening sheet
  cancels the drag. A single terminal has no header, so it moves through the
  command palette's "Move terminal to …" commands.
- **Pane status.** Starting, exited, failed and resize-error states are shown by
  a centred capsule near the pane's bottom edge with one action. A successful
  exit is stated plainly; only failures use the problem colour. A remote
  terminal offers "Reconnect" where a local one offers "Restart". "Back to
  bottom" appears at the bottom trailing corner while scrolled into history.
- **Notifications.** An unread OSC alert rings its pane in amber: a crisp edge
  with a soft inner glow that fades in over 160 ms and takes the focus ring's
  place rather than doubling it; a single pane shows it too. The workspace row
  brightens its name, shows the latest alert beside a small bell in place of
  the path, and carries a tinted count pill; a collapsed folder sums its
  workspaces. The toolbar bell carries a dot cut out of the glyph and stays
  pressed while its popover is open. The popover floats in the top trailing
  corner where messages do, without dimming the window or resizing shells.
  Its header counts unread alerts and offers "Mark all read" and "Clear all".
  Rows are newest first: the workspace's identity tile (with the unread dot),
  the alert in up to two lines each of title and body, its workspace and
  program, and its age, which yields to a dismiss control under the pointer.
  The whole row opens the originating pane. Nothing is highlighted until an
  arrow key is pressed; then Up and Down move through the alerts, Enter opens
  one and Delete dismisses it. An empty history says so plainly. Desktop
  banners are optional; [notification behavior and agent
  setup](notifications.md) describes acknowledgement, lifetime and protocol
  limits.
- **Workspace creation.** New workspace immediately opens and selects a fresh
  shell in the home directory (`~`). Its name can be changed afterward from the
  sidebar or command palette. A new SSH workspace asks only for its host and
  is named after it.
- **Sheets.** Preferences, rename, SSH connection and close confirmation are
  centred modal sheets over a dimmed window; the dim follows the window's
  rounded shape. The confirming action sits at the trailing edge; destructive
  confirmations are red and are never the Enter default. Each close target has
  its own title and consequence ("Close terminal?", "Close workspace?",
  "Disconnect from SSH?", "Quit Neptune?"). The SSH sheet asks only for a host,
  states that connecting an existing workspace restarts its terminals, and
  names the problem in place of that note while the host is unusable.
- **Software update.** The sheet leads with the version on offer, the
  installed version and a link to the release. The release notes scroll
  beneath as headed lists that wrap to the sheet, followed by how to install.
  Progress, a verified download or a failure sits in a card above the action
  bar and stays in view while the notes scroll; "Later" and the download
  action trail the bar.
- **Preferences.** A source list beside one pane of settings, as a desktop
  settings window is laid out. The list is a surface of its own in the window
  material, concentric with the sheet, and names the panes with an icon:
  General, Appearance, Text, Shell, Notifications and Updates. The selected
  pane is marked as a selected workspace is, its name titles the content, and
  the close control trails that title. Each pane is a short column of labelled
  cards: rows with the control at the trailing edge, and the note that explains
  them inside the same card, under a hairline. Settings apply immediately, so
  there is no confirming action; "Reset to defaults" is a row in General and
  retains the custom themes and the favorites. General's Keyboard card opens
  the active config file and explains that custom keybindings require a restart.
  Its Shortcut reference accordion reveals selectable config names, platform
  defaults and accepted shortcut syntax within the same card. The full header
  is clickable, with a chevron indicating whether the reference is expanded.
  It is also found by searching for shortcuts or keybindings. The sheet keeps
  one height for every pane and remembers the pane in view while the app runs.
  A sheet too narrow for names keeps the list as icons with tooltips.
  A search field leads the source list and takes the keyboard when the sheet
  opens; in a narrow sheet it takes the title's place. Typing replaces the
  pane with "Results": the matching rows as working controls, in cards named
  for their pane and section, while each pane in the list counts what was
  found in it and the others recede. Every word typed must match a setting's
  label, its pane or another common word for it ("dark mode" finds the theme,
  "caret" the cursor); a misspelling is accepted only when nothing matches as
  typed. Enter opens the pane of the first result, choosing a pane leaves the
  search, and Escape clears the search before it closes the sheet.
  Appearance leads with the theme in use: a miniature drawn in its own colours,
  its name and where it comes from. The whole row opens the theme catalog.
  Below it is a window zoom percentage stepper. Text has an installed monospace
  font family selector with a pinned "Custom…" choice that opens a focused name
  field. Enter applies the name; partial names leave the terminal font in place.
  When an abbreviation or PostScript name resolves to a different installed
  family name, the font card identifies the family being used.
  A font size stepper has its Ctrl+Shift+Plus/Minus and Ctrl+Shift+0 shortcuts (Command+Shift on
  macOS), a slider for line spacing, a segmented cursor style and the blink
  switch. Boolean settings are switches.
- **Themes.** The catalog and the editor are screens of the Preferences sheet.
  A back control leads the title, and they take the sheet's full width in
  place of the source list. The search field takes the keyboard and shares a line with the
  All/Dark/Light/Custom/Favorites filter. Cards are grouped as "Favorites",
  "Your themes", "Neptune" and "iTerm2 collection"; each is a miniature of the
  window with sample text in the theme's colours, never terminal contents. Only
  the rows in view are laid out. The catalog opens at the theme in use, which
  carries the accent ring and a check; choosing a card applies it to the window
  and every terminal at once. On hover or focus a card shows a "more" control
  with the same menu as a secondary click: "Use theme", "Add to favorites" or
  "Remove from favorites" and "Duplicate…", and for a custom theme "Edit…" and
  "Delete…". Deleting asks in the action bar, in place of "New theme" and
  "Done", so nothing reflows.
  A star at the end of a card's name keeps the theme as a favorite without
  applying it. A favorite's star is always shown, in the accent; any other
  card shows a quiet one on hover or focus. Favorites lead the catalog in the
  order they were starred and stay listed where they come from; the Favorites
  filter shows them alone, and a search lists each theme once. A card starred
  where it stands stays under the pointer while the section above it changes.
  The editor keeps the name and a live preview beside the colours while there
  is room and stacks them in a narrow window. Colours are grouped rows: text
  and background, cursor and selection, and each ANSI colour beside its bright
  variant. A colour well opens a picker (a saturation and brightness square
  over a hue strip); its hex field accepts `#RRGGBB`, `RRGGBB` or `#RGB`. A
  value that cannot be used is outlined in the problem colour and named in the
  action bar, and "Save theme" waits for it. Nothing reaches the configuration
  before Save. A changed draft is never dropped silently: Cancel, back, Escape,
  the close control, the preferences shortcut and a click outside the sheet
  ask in the action bar first, and Escape answers "Keep editing". A draft set
  aside by another overlay is still there when Preferences reopens. Escape
  leaves the innermost thing first: the picker or a question, then the editor,
  the catalog and the sheet.
- **Command palette.** Every action with its shortcut, grouped when browsing and
  flat when filtered. Arrow keys move the highlight, Enter runs, and the pointer
  only takes the highlight when it moves. Commands that need a terminal are
  listed only when one is focused, and each captures its target when drawn.
- **Menus and tooltips.** Menus highlight the hovered row in the accent, show
  shortcuts at the trailing edge and mark destructive items in red. Tooltips
  name the control and its shortcut.

The default launch state is an actual shell. An empty window offers one action;
it does not show onboarding, sample data or decoration.

## Type and icon rhythm

Interface text is Geist in three weights: regular for body and values, medium
for names, labels and buttons, semibold for sheet titles and tiles. Sizes are
13 for body and names, 12–12.5 for secondary and control text, 11–11.5 for
section labels and paths, and 15 for sheet titles and the palette field.
Terminal text defaults to bundled JetBrains Mono; Preferences → Text selects an
installed monospace family for every terminal. Preferences or Ctrl+Shift+Plus/Minus
(Command+Shift on macOS) changes its size; Ctrl+Shift+0 (Command+Shift+0 on
macOS) resets it to the default. App zoom
uses Ctrl+Plus/Minus (Command on macOS), with Ctrl+Equals as an unshifted Plus
alternative and Ctrl+0 to reset (Command on macOS). App zoom scales terminal
text and chrome together without changing the saved terminal font size. Window
zoom also appears under Appearance in Preferences and persists across launches,
whether changed there, by shortcut or from the command palette. A shortcut or
palette command that steps either one names the result in a capsule centred
under the toolbar, such as "Font size 15 pt" or "Window zoom 110%", also at the
end of a range where the value stays. It appears at once, takes no input, holds
for 1.2 s and fades over 160 ms; the window sleeps through the hold. Labels
use sentence case.

Icons are drawn natively on a 24-point grid with a 1.5-point stroke and rounded
ends, matching regular-weight text. Recolour an icon for state; do not swap
assets. An agent's mark is the exception to native drawing: it is the agent's
own artwork, bundled as a mask and tinted like an icon, except Claude Code's,
which keeps its orange. Every icon-only control has a tooltip and an accessible name that match.

## Bundled fonts and notices

The application bundles unmodified static TrueType editions in `assets/fonts`,
fetched from the official Geist and JetBrains Mono repositories: Geist Regular,
Medium and SemiBold, and JetBrains Mono Regular and Bold. Their SIL Open Font
License notices and SHA-256 hashes are documented there. The bold terminal face
preserves the regular face's cell width. Static editions provide predictable
native metrics without a WOFF decoder; conversion or bundling must preserve each
font's copyright and complete license notice.

Installed Nerd, symbol and CJK fonts are loaded as fallbacks from known system
locations, once per process, for every interface weight and for the terminal.
They are not redistributed. This supports the development machine's shell
prompt without replacing the bundled Latin grid metrics; it does not establish
comprehensive shaping or font discovery on every platform. Missing glyphs and
complex-script behaviour need native verification.

## Screenshot review

Review the running native application, not a static mockup. For a routine UI
change, capture the affected states after the final build, including a narrow
window. Check that terminal type stays legible, the focused pane is
unambiguous, icon targets share a baseline, nothing clips, and overlays keep
focus. The terminal's output should hold more visual weight than the chrome.

Release visual acceptance requires fresh native captures after the last build,
covering single and three-pane layouts, workspace creation, Graphite and Light
preferences, search, the terminal menu, close confirmation, and 900×640 and
640×480 logical windows. A successful interaction report alone does not approve
the visual design. The evidence and remaining release gates are described in
[verification.md](verification.md).

### Review record: 2026-09-30 rebuild

Reviewed on Linux (X11 through the inspection harness, 1 px per logical point)
from fresh captures of the rebuilt interface: single pane, three panes, hidden
sidebar, zoom, search, command palette (browsing, filtered and without a
terminal), preferences in all three themes with a non-default accent, new
workspace, both menus, tooltips, close and quit confirmation, exited and failed
terminals, the error message, the empty window, and 900×640 and 640×480 windows.

Corrections made during that review:

- Search first floated over the pane and could cover a match; it moved into the
  toolbar.
- The error message first sat at the bottom centre and hid a failed pane's
  status; it moved to the top trailing corner.
- Menus stretched to the widest space offered; they now have a fixed width.
- A dialog's first field did not take the keyboard, because focus was requested
  during the sheet's hidden measuring pass.
- Escape that closed a menu was also delivered to the shell.
- A placeholder pane (starting or failed) drew a cursor; it no longer does.

An independent code review then found focus edge cases, all corrected: Tab or
an arrow pressed in the frame after the terminal regained focus could move
focus into the chrome; the palette re-requested focus every frame, which
interrupts input-method composition; same-named workspaces shared a palette row
identity; the search buttons handed the keyboard to the shell while their
tooltips promised Enter; a message swallowed one Escape meant for the shell; a
held Enter could confirm a dialog before it appeared; and double-click on the
split and sidebar handles was not sensed.

This record covers the recorded Linux states. Wayland input, macOS and Windows
still require native design review, as do the window controls' platform fit.

### Review record: 2026-10-02 theme catalog

Reviewed on Linux (X11 through the inspection protocol, 1 px per logical point,
debug build with the `inspection` feature) from fresh captures after the last
build: preferences with a Neptune, an imported and a custom theme; the catalog
at the top, opened at a theme deep in the list, searched, filtered, with no
results and with no custom themes; a hovered card and its menu; the removal
question; the editor for a new and a saved theme with a dark and a light
palette, an unusable value, the picker and the discard question; and the same
screens at 640×480 and at 150% window zoom in a 640×480 window.

Corrections made during that review:

- The catalog stacked a back button, search, filter, a create button and a
  count above the grid, leaving one row of cards in a small window. Search and
  filter now share a line and the actions moved to the action bar.
- Editing or deleting a custom theme first required applying it. Every card
  now has a menu.
- The editor's Save and Cancel sat above the form and its colour rows did not
  use the sheet's width; the preview scrolled away from the colours it showed.
- Escape, the close control and a click outside the sheet discarded a draft
  without asking.
- Destructive buttons drew white labels on the light reds of imported
  palettes; the label now follows the red's contrast.
- The catalog opened at the top however far down the theme in use was.

On native Wayland, preferences, the catalog, a search and the editor were
captured once from the same build and matched. The remaining states there,
macOS and Windows were not reviewed for these screens.

### Review record: 2026-10-02 theme favorites

Reviewed on Linux (X11 through the inspection protocol, 1 px per logical point,
debug build with the `inspection` feature) from fresh captures after the last
build: the catalog without favorites; a hovered card, its star and the star's
tooltip; a card starred where it stands; the favorites leading the catalog in
Light, Graphite and a custom theme; the Favorites filter with and without
favorites; both menu items; a search; a long name on the theme in use; the star
focused and toggled from the keyboard; a starred custom theme and its removal;
and the catalog at 640×400 and at 150% window zoom.

Corrections made during that review:

- A favorite's star was first a filled glyph, which swapped the icon for
  state. It is the one outline star, recoloured in the accent.
- Five filters at the width of four crowded "Favorites" against the control's
  edge. The filter is wider and moves under the search field sooner.
- The star's focus ring touched the ring of the theme in use; its hover and
  focus surfaces are inset.

On native Wayland, the catalog and the Favorites filter with a hovered star
were captured once from the same build and matched. macOS and Windows were not
reviewed for this change.
