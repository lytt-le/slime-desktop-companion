# Slime companion

## Work directly on main

This is a pet project: no branches, worktrees or pull requests. Make changes in the main
checkout (`G:\Claude Projects\companion`) on `main`, commit there and push to
`origin main`.

## Keep the local binary up to date

After any code change, rebuild so the `.exe` matches the code:

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
