# Releasing CrabClaw

Use this checklist for reliable production releases.

## 1) Preflight (main branch)

- [ ] `cargo test --locked`
- [ ] `cargo clippy --locked --all-targets`
- [ ] `bash scripts/benchmark_ci.sh`
- [ ] CI green on `main`

## 2) Tag release

```bash
git checkout main
git pull --ff-only origin main
git tag -a vX.Y.Z -m "CrabClaw vX.Y.Z"
git push origin vX.Y.Z
```

## 3) Verify Release workflow

- Open Actions → **Release**
- Confirm all target builds succeed
- Confirm **Publish Release** runs successfully
- Confirm release is **not draft** and marked **latest**

## 4) Verify assets

Expected minimum assets:

- `crabclaw-x86_64-unknown-linux-gnu.tar.gz`
- `crabclaw-x86_64-unknown-linux-musl.tar.gz`
- `crabclaw-x86_64-pc-windows-msvc.zip`
- `crabclaw-x86_64-apple-darwin.tar.gz`
- `crabclaw-aarch64-apple-darwin.tar.gz`
- `SHA256SUMS`

## 5) Verify installer path

- [ ] `https://github.com/NaritATK/CrabClaw/releases/latest` redirects to the new tag
- [ ] Linux install works:

```bash
curl -fsSL https://raw.githubusercontent.com/NaritATK/CrabClaw/main/scripts/install.sh | bash
```

- [ ] Musl install works:

```bash
curl -fsSL https://raw.githubusercontent.com/NaritATK/CrabClaw/main/scripts/install.sh | bash -s -- --musl
```

## 6) Post-release

- [ ] Update CHANGELOG (if used)
- [ ] Announce release notes
- [ ] Keep previous tag available for rollback
