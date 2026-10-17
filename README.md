# Slime Desktop Companion

A gooey pixel-art slime that lives on your Windows desktop. He sits on your windows and taskbar, gets thrown around, chats with you through Claude or DeepSeek, and gets up to mischief when left alone. Written in Rust on raw Win32 (no engine), simulated Noita-style: the desktop is a grid of cells (3 screen pixels each), and the slime is ~900 individual cells that each move and collide on their own. Goo drips and bubbles are cells too.

## Controls

- **Left-click + drag** the slime: pick him up; release to drop or fling him
- **Left-click anywhere else**: he hops toward the click (toggle with *Jump to clicks*)
- **Right-click** the slime or the tray icon: the **settings panel** below
- **Tray icon → double-click**: reset

## What he gets up to

He lands on and sits on visible window title bars and the taskbar, drapes and slides off edges, and rides along when you move the window he's sitting on. A window edge he's straddling (e.g. after being thrown up through the window) doesn't catch him. With several monitors he stays on the visible screens, never in the gaps between differently sized ones. He leaves goo behind that sticks where it lands and fades, and air bubbles drift around inside him and pop at the surface.

**Idle antics**: every 5 to 17 seconds while he's sitting about on his own, he does something by himself: inches along for a few seconds (eyes looking where he's going), does one to three little hops, squashes himself flat and springs back, stretches up tall, wobbles side to side, lets off a burst of bubbles, or glances left and right. He turns round at the edge of a window or the screen rather than falling off, and stops the moment you grab him or click somewhere for him to jump to.

Now and then he takes a **smoke break** (half-lidded eyes, a cigarette with a glowing tip that flares on each drag, smoke trickling up and curling, and a puff breathed out every few seconds; grabbing him makes him drop it), or, if he's been left alone for 40 seconds or more, a **nap** (he flattens a little, closes his eyes and Z's float up from his head; grabbing him, clicking for a jump, opening the panel or a chat reply wakes him, otherwise he wakes by himself with a stretch).

**Moods**: flung really hard into a wall (a screen edge, hit at about 2800 px/s or more; ordinary throws and bumps don't count), he scowls with cross brows for a couple of seconds. Shaken hard in your hand (vigorously, about four shakes a second or faster; carrying him about or a gentle jiggle doesn't count), he gets dizzy: spinning spiral eyes, and he reels from side to side for a few seconds after you put him down.

**Fullscreen**: when something goes fullscreen on his monitor (F11 in a browser, a fullscreen video, a borderless game), he gets out of the way. With another monitor free, he leaps over to it, in an arc high and fast enough to clear the edge between the screens (after three leaps that fall short he just drops in there). With no free monitor, he stays where he is but tucks in behind the fullscreen window, out of sight, with no fun facts and no idle antics, and pops back on top when it leaves fullscreen. Clicks inside a fullscreen app don't make him jump. Only real fullscreen counts: a window covering the whole monitor, taskbar included, with no title bar; maximized windows (even with an auto-hiding taskbar) don't.

## Settings panel

A pixel-art panel with three tabs that opens at the cursor; click anywhere else or press Esc to close it. Everything is remembered in `%APPDATA%\SlimeCompanion\settings.json`.

The **Chat** tab is always the one showing when it opens. Just type: Enter sends, Ctrl+V pastes, Backspace deletes. The conversation scrolls in the log above (your messages after a `>`), he answers there and in his speech bubble, and he remembers the last few exchanges. The bottom line says which brain is answering. Opening the panel also starts Claude Code, so the first answer is quick.

**Ask about this page** takes a screenshot of the window you were looking at when you right-clicked him (or the whole monitor, if that was the desktop or taskbar). A thumbnail sits above the input until your next question goes with it; **x** removes it, and pressing Enter with nothing typed just asks what's on screen. The slime, the panel and his bubble are left out of the picture (they're excluded from capture only for that instant, so your own screenshots still show him). The image is scaled to at most 1568 px on its long side and sent as PNG (JPEG if it would be too big). All three brains can see it; on DeepSeek, image questions go to V4.1 Flash, the DeepSeek model that takes images.

The **Settings** tab:

- **Size**: drag the slider (50%-250%; it snaps to 100% at the notch).
- **Jump to clicks**, **Fun facts** (the facts he shares every minute or two), **Idle antics** (everything in *Idle antics* above, including smoke breaks and naps).
- **Brain**: click `<` / `>` (or the value) to cycle the **Provider** and **Model**:

  | Provider | Models | Needs |
  |---|---|---|
  | Claude plan | Haiku, Sonnet, Opus | [Claude Code](https://claude.com/claude-code) installed and signed in with your Pro/Max plan |
  | Anthropic API | Haiku 5.5, Sonnet 5.5, Opus 5.5 | an Anthropic API key (`sk-ant-...`) |
  | DeepSeek API | V4.1 Flash (`deepseek-flash`), V4 Pro (`deepseek-v4-pro`) | a DeepSeek API key (`sk-...`) |

- **Search the web** (on by default) lets him look things up when a question needs current or specific facts: news, weather, prices, recent releases. While he searches, his bubble says what he's looking for ("Searching for ...", "Reading example.com..."). Untick it and he answers from what the model already knows.
- **Key**: copy an API key, then press **Paste** (or paste it into the Chat tab and press Enter). **Clear** forgets it. Keys are saved in plain text as `api_key.txt` / `deepseek_key.txt` in the same folder; `ANTHROPIC_API_KEY` / `DEEPSEEK_API_KEY` are used when nothing is saved.
- **Reset slime**, **Quit**.

The **Customisation** tab changes how he looks and feels, live. Each slider has a notch at the classic slime; sliders snap to it, and **Back to the classic slime** resets them all.

- **Colour**: nine preset swatches (green, teal, blue, purple, pink, red, orange, gold, grey), or the **Colour** wheel, **Vivid** (grey to extra-saturated) and **Clarity** (see-through to solid).
- **Feel**:
  - **Runny**: thick and tall, through to a flat, runny puddle (viscosity, flow, dome shape).
  - **Wobble**: firm, through to very jiggly (jelly damping and how hard impacts kick it).
  - **Splatter**: how much goo comes off, from none at all to lots (and in bigger blobs).
  - **Trail**: how long dropped goo lasts (about 1 to 30 s).
  - **Bubbles**: none, through to a fizzy 20.
  - **Grip**: slippery, through to sticky (sliding friction).
  - **Weight**: floaty, through to heavy (gravity).

Goo matches the slime and how rough things get: hard landings and wall hits throw round blobs that fly off and splat (more and bigger the harder the hit, and bigger on a bigger slime), sliding lays a smear behind him (dotted when slow, several cells thick when fast), and he drips small drops while carried or flying.

### How chat reaches a model

- **Claude plan**: the slime runs Claude Code headless (`claude -p` with `--input-format stream-json`) and keeps that one process open while you chat, so only the first answer pays for startup (about 2 s; later ones about 1 s). Its only tools are WebSearch and WebFetch (and none at all with Search the web off): no files, shell, MCP servers, skills or saved transcripts. It closes after 15 minutes without a question, when you change provider or model, or when the slime quits (it sits in a kill-on-close job, so it can't outlive the slime). Questions count toward your plan's usage limits. This uses your own Claude Code login on your own machine; it is not a way to share a subscription.
- **Fallback**: if you pick an API provider but haven't added its key yet, the Claude plan answers instead (when Claude Code is installed). The panel's status line says so.
- **APIs**: direct HTTPS calls over WinHTTP. Anthropic uses the Messages API at low effort; DeepSeek uses `/chat/completions` with thinking turned off for quick replies. Every brain is told today's date.
- **Web search**: on Anthropic, the server-side `web_search` and `web_fetch` tools (the `_20260209` versions with dynamic filtering; plain `web_search_20250305` on Haiku). Searches are billed per search on top of tokens. If the API turns the tools down, he answers without them. On DeepSeek, the model gets `web_search` and `fetch_page` functions, which the slime runs against DeepSeek's Search (`/v1/tools/search`) and Fetch (`/v1/tools/fetch`) APIs with the same key, for up to 4 rounds before it must answer. DeepSeek plans to bill those at $0.75 per 1,000 successful calls from 1 Nov 2026. On the Claude plan, Claude Code's own WebSearch/WebFetch, counted toward plan usage.

## Build & run

```sh
cargo run --release
```

The exe ends up at `target/release/slime.exe` (no runtime dependencies), with the slime icon (`assets/slime.ico`) embedded by `build.rs`. To redraw the icon from the current renderer: `cargo test --release write_exe_icon -- --ignored`, then rebuild.

### If something goes wrong

The exe runs itself as a worker under a tiny guardian. If the worker crashes, the guardian starts a fresh slime ("Whoops! Something went splat, but I'm back!") and appends the panic to `%APPDATA%\SlimeCompanion\crash.log`. Quitting from the panel, or ending it in Task Manager, stops it for good; after more than 5 crashes in a minute it gives up. He also respawns by himself if he's ever off every screen, or mostly lost, for more than a second. Set `SLIME_NO_GUARD=1` to run without the guardian (for debugging), or `SLIME_CRASH_TEST=1` to make the first worker crash after 3 s and check that he comes back.

### Testing

- `cargo test --release` runs headless scenario tests: landing, corner tip-off, drape, throws, violent shaking, jumps, resizing, sliding, straddled window edges, monitor gaps, leaps between monitors, idle antics, moods, the panel and the speech bubble. They also write contact sheets of rendered frames to `target/snapshots/*.bmp`.
- `cargo test --release -- --ignored` also runs the live checks: fake-key round trips to both APIs, a two-question Claude Code session on your plan, Claude reading a picture through that session, a web search through Claude Code and one through DeepSeek with your saved key, screenshots leaving out the slime's windows (a small magenta box flashes in the top-left for a moment), and an invisible borderless window over the main monitor being spotted as fullscreen with the slime tucking behind it.
- `tools/e2e.ps1` drives the **real mouse** against a running slime and records a contact sheet, e.g.

  ```powershell
  powershell -ExecutionPolicy Bypass -File tools\e2e.ps1 -Scenario throw -Follow "300,200" -Out throw.png
  ```

  Scenarios: `idle`, `drag`, `shake`, `throw`, `throw_up`, `drop_window`, `jump`. `-Follow w,h` records a box that follows the slime at full size; `-Crop x,y,w,h` records a fixed region.

## Tuning

- `src/sim.rs`: squishiness is the `JELLY_*` constants: `JELLY_HZ` (wobble speed), `JELLY_DAMPING` (lower = wobbles longer), `JELLY_LAND` / `JELLY_WALL` / `JELLY_BRAKE` / `JELLY_CARRY` / `JELLY_JUMP` / `JELLY_GRAB` (how hard each event squashes, stretches or rocks it). Also `DOME_ASPECT` (resting height vs width), `RESHAPE_RATE` (how fast the goo flows into its shape), `GROUND_FRICTION`, `BUBBLE_RISE`. Idle antics: `WALK_SPEED` / `WALK_HZ` (pace and steps per second), `HOP_CROUCH` / `HOP_APEX` / `HOP_LEN` (the little hops), `WOBBLE` (how hard he sways).
- `src/fx.rs`: `ANGRY_HIT` (how hard a wall hit makes him cross), `DIZZY_SHAKE` (how much shaking makes him dizzy), and the Z and smoke timings.

## How it works

- `main.rs`: per-pixel-alpha layered windows (`UpdateLayeredWindow`) for the body (sized to the slime each frame; transparent pixels pass clicks through), goo, the speech bubble and effects; a low-level mouse hook for click-to-jump; the tray icon and panel; idle antics, moods and fullscreen handling; and the frame loop (60 fps, 30 fps when settled).
- `guardian.rs`: restarts the slime after a crash and logs panics.
- `desktop.rs`: enumerates top-level windows in z-order and clips each title bar against windows above it, so the slime only stands on parts you can see; also reports monitor rectangles, work-area floors, and fullscreen windows.
- `physics.rs`: small 2D vector, platform and bounds types.
- `sim.rs`: the cell world. Each substep:
  - **Whole-body motion**: gravity, throws, jumps and drags move every body cell together one cell at a time, front cells first. Cells that hit something stay put, so the body squashes around obstacles. If enough of the body is blocked it has landed (or hit a wall) and that motion stops dead.
  - **Local motion**: each cell's own velocity on top. Cells feel gravity minus the body's acceleration (weightless in free fall, a shove when the body is stopped or yanked, so landings splat), plus cohesion toward the centre and viscosity with their neighbours.
  - **Liquid flow** while moving (blocked falls slide diagonally or sideways).
  - **Jelly**: the body's shape is two damped springs, squash/stretch and lean. Landings, wall hits, braking, carrying, jumps and grabs kick them, so it splats, springs back, wobbles and settles (and gently breathes at rest). Idle antics drive the same springs.
  - **Flowing into shape**: cells sticking out of the target shape (a dome when resting, an oval in the air, bent by the jelly springs) flow in like an incompressible liquid. Every cell on the line to the nearest free spot shifts one step, so the goo visibly moves through itself. Kawasaki surface tension smooths the surface, and enclosed gaps collapse.
  - **Window edges**: one-way, judged by the body's lowest cell like a platformer's feet.
  - **Goo**: drips while carried or flying; round blobs thrown off by hard impacts (sized by the slime's radius and the impact speed, made just outside the surface from cells taken off it, and moved front-first so they fly whole); and a smear laid behind a slide, its thickness set by speed and size. Goo falls, sticks, fades, and is soaked back up on contact.
  - **Bubbles**: air cells that swap upward through the slime and pop at the surface.
- `fx.rs`: moods and extras: cross brows, dizzy spirals, sleep and its Z's, the cigarette and its smoke (drawn in their own click-through window, since they stick out past the body).
- `brain.rs`: fun facts, and chat on a worker thread through the chosen provider: a persistent Claude Code session, or the Anthropic / DeepSeek APIs over WinHTTP. Short in-character replies; friendly messages for bad keys, sign-in, usage limits, rate limits, overload and refusals; web search (Claude Code's tools, Anthropic's server tools, or DeepSeek's Search/Fetch APIs run as functions); a speech queue that types replies out a page at a time.
- `shot.rs`: "Ask about this page": finds the window in front, captures it (keeping the slime's windows out of the shot), scales it, encodes PNG/JPEG with GDI+, makes the thumbnail.
- `settings.rs`: the panel's choices, saved as JSON, and how the Customisation sliders map onto the sim's `Tuning` and the renderer's `Palette` (the middle of every slider is the classic slime).
- `font.rs` / `ui.rs`: a 5x7 (+ descender) bitmap font, and the pixel speech bubble and the tabbed panel (chat log and input, buttons, sliders, checkboxes, selectors, swatches, hit-testing), drawn in chunky pixels to match the slime.
- `draw.rs`: each cell drawn as a solid square with neighbourhood-based pixel-art shading: dark outline, lit top rim, depth gradient, shadowed underside, a shine sprite, bubble rings, a per-cell tint that travels with the goo so you can see it flow, and the eyes in whatever look his mood calls for.
