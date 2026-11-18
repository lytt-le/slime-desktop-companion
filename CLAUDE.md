# Slime companion

## Work directly on main

This is a pet project: no branches, worktrees or pull requests. Make changes in the main
checkout (`G:\Claude Projects\companion` on Windows, `~/Projects/slime-desktop-companion` on
Linux) on `main`, commit there and push to
`origin main`.

## Keep the local binary up to date

After any code change, rebuild so the binary matches the code.

### Linux (KDE Plasma 6, Wayland)

Checkout: `~/Projects/slime-desktop-companion`.

```sh
cargo test --release
cargo build --release
cargo check --release --target x86_64-pc-windows-gnu --all-targets   # Windows side still compiles
```

- That updates `target/release/slime`, which the app-menu entry (`~/.local/share/applications/slime-companion.desktop`) runs.
- To restart it: `pkill -x slime` (it quits tidily on SIGTERM, unloading its KWin script), then start it again
  detached, e.g. `setsid -f target/release/slime`. `SLIME_NO_GUARD=1` runs it without the guardian.
- Don't `pkill -f` a pattern that's also in your own command line: it kills the shell running it.

### Windows

Rebuild so the `.exe` matches the code:

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test --release
cargo build --release
```

- That updates `G:\Claude Projects\companion\target\release\slime.exe`, the one the user runs.
- If the build can't overwrite `slime.exe`, it's running: close it (`Stop-Process -Name slime`;
  the second process is his guardian), rebuild, then start it again.
- The GitHub release download (tag `V1`) is only updated when asked:
  `gh release upload V1 target\release\slime.exe --clobber -R lytt-le/slime-desktop-companion`.
