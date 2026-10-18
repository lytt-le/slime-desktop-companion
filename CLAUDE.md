# Slime companion

## Keep the local binary up to date

After any code change, rebuild so the `.exe` matches the code:

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test --release
cargo build --release
```

- Working in a worktree (`.claude/worktrees/...`), that build lands in the worktree's own
  `target\release\slime.exe`. The one the user runs is in the main checkout,
  `G:\Claude Projects\companion\target\release\slime.exe`. After a merge, `git pull`
  there and run `cargo build --release` in it too.
- If the build can't overwrite `slime.exe`, it's running. Say so; don't kill it.
- The GitHub release download (tag `V1`) is only updated when asked:
  `gh release upload V1 target\release\slime.exe --clobber -R lytt-le/slime-desktop-companion`.
