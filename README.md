# herdr


<p align="center">
  <img src="assets/logo.png" alt="herdr" width="100" />
</p>

<p align="center">
  <a href="https://herdr.dev">herdr.dev</a> · <a href="#install">install</a> · <a href="https://herdr.dev/docs/quick-start/">quick start</a> · <a href="https://herdr.dev/docs/">docs</a>
</p>

<p align="center">
  English · <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-666666?labelColor=333333" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/shuv1337/herdr/releases"><img src="https://img.shields.io/github/downloads/shuv1337/herdr/total?labelColor=333333&color=666666" alt="total GitHub release downloads" /></a>
  <a href="https://github.com/herdrdev/herdr/stargazers"><img src="https://img.shields.io/github/stars/herdrdev/herdr?labelColor=333333&color=666666&logo=github" alt="GitHub stars" /></a>
  <a href="https://github.com/shuv1337/herdr/releases/latest"><img src="https://img.shields.io/github/v/release/shuv1337/herdr?label=fork%20release&labelColor=333333&color=666666" alt="latest stable release" /></a>
  <a href="https://formulae.brew.sh/formula/herdr"><img src="https://img.shields.io/homebrew/v/herdr?label=homebrew&labelColor=333333&color=666666" alt="Homebrew version" /></a>
  <a href="https://x.com/herdrdev"><img src="https://img.shields.io/badge/follow-%40herdrdev-000000?logo=x&logoColor=white" alt="follow @herdrdev on X" /></a>
</p>

---

> [!NOTE]
> **This is the [shuv1337/herdr](https://github.com/shuv1337/herdr) fork of [herdrdev/herdr](https://github.com/herdrdev/herdr).**
> It tracks upstream `master` and adds fork-only changes such as shuvcode/shuvpi integrations, native jj workspaces, custom local machine labels, and the night-owl-gold theme.
> Each [release](https://github.com/shuv1337/herdr/releases) lists the fork's changes in its notes.
> Fork builds are versioned `<upstream-version>-shuv.<N>` (for example `0.9.1-shuv.1`), and `herdr update` installs updates from this fork's releases, not from herdr.dev.
> Report fork-specific problems [here](https://github.com/shuv1337/herdr/issues), not upstream.

https://github.com/user-attachments/assets/043ec09f-4bdd-41d5-aee0-8fda6b83e267

**the runtime your coding agents live on.**

- **detach without stopping work** — herdr keeps terminals running in a background server when you close the client or lose your SSH connection. after a server or machine restart, herdr restores the saved layout and can resume supported agent sessions; the original processes do not survive. [session state →](https://herdr.dev/docs/session-state/)
- **several machines, one window** — keep local work and saved ssh machines together, with a combined agent list and independent reconnects. [remote machines →](https://herdr.dev/docs/connecting-machines/)
- **never hunt for the stuck one** — every pane is marked working, blocked, or idle. when an agent stops and needs an answer, herdr says so.
- **agent-native** — agents drive herdr through the cli and socket api: they can spawn panes, prompt each other, and wait until another agent is genuinely blocked. [agent skill →](https://herdr.dev/docs/agent-skill/)
- **runs what you already run** — claude code, codex, cursor, opencode, grok and the rest. herdr doesn't wrap or replace them; it owns their terminals. building an agent? [add herdr support →](https://herdr.dev/docs/add-herdr-support/)
- **keyboard and mouse, both first-class** — tmux-style prefix keys *and* click, drag, split. pick per moment, not per tool.
- **plugins** — extend panes and workflows. [browse the marketplace →](https://herdr.dev/plugins/)
- **one rust binary, no electron** — runs in whatever terminal you already use.

---

## install

install the fork on linux or macos:

```bash
curl -fsSL https://github.com/shuv1337/herdr/releases/latest/download/install.sh | sh
```

windows (powershell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/shuv1337/herdr/releases/latest/download/install.ps1 | iex"
```

or download a binary from [releases](https://github.com/shuv1337/herdr/releases). the installer puts `herdr` in `~/.local/bin` (set `HERDR_INSTALL_DIR` to change that) and replaces any herdr already installed there. fork installs update with `herdr update`. this fork publishes stable releases only, so leave `herdr channel` set to `stable`.

`brew install herdr`, `mise use -g herdr`, Nix, and `https://herdr.dev/install.sh` install **upstream** herdr without the fork's changes. to switch back to upstream, run `curl -fsSL https://herdr.dev/install.sh | sh`.

then start it where the work lives:

```bash
herdr
```

run your agents, split panes, walk away. `ctrl+b q` detaches, `herdr` reattaches. [quick start →](https://herdr.dev/docs/quick-start/)

## docs

everything lives at [herdr.dev/docs](https://herdr.dev/docs/): [quick start](https://herdr.dev/docs/quick-start/) · [concepts](https://herdr.dev/docs/concepts/) · [supported agents](https://herdr.dev/docs/agents/) · [keyboard](https://herdr.dev/docs/keyboard/) · [configuration](https://herdr.dev/docs/configuration/) · [session state](https://herdr.dev/docs/session-state/) · [connecting machines](https://herdr.dev/docs/connecting-machines/) · [remote](https://herdr.dev/docs/persistence-remote/) · [integrations](https://herdr.dev/docs/integrations/) · [add herdr support to your agent](https://herdr.dev/docs/add-herdr-support/) · [plugins](https://herdr.dev/docs/plugins/) · [socket api](https://herdr.dev/docs/socket-api/)

## thanks

every past sponsor and backer is listed in [SPONSORS.md](./SPONSORS.md) — thank you 🐑

enterprise / partnership: hey@herdr.dev

## agent instructions

if you are an ai agent helping with this repository, read [`AGENTS.md`](./AGENTS.md) before making changes and read [`CONTRIBUTING.md`](./CONTRIBUTING.md) before opening issues or PRs.

## development

```bash
git clone https://github.com/shuv1337/herdr
cd herdr
cargo build --release

just test        # unit tests
just check       # formatting, tests, and maintenance checks
```

### fork releases

fork releases are built by [`.github/workflows/fork-release.yml`](.github/workflows/fork-release.yml) from `v<cargo-version>-shuv.<N>` tags. to publish one, push `master`, then run:

```bash
just fork-release            # tags origin/master as the next v<version>-shuv.<N> and pushes the tag
just fork-release --dry-run  # print the next tag without creating it
```

the workflow builds all five platform assets and attaches them to the GitHub release, along with `latest.json` (the update manifest `herdr update` reads) and the install scripts. after publishing, it posts a short announcement to Discord through the `DISCORD_RELEASE_WEBHOOK_URL` repository secret. if the secret is missing, it skips the post; if the post fails, the release still succeeds. it does not commit anything back to `master`, so syncing upstream won't conflict with fork releases.

## license

Herdr is licensed under the [Apache License 2.0](LICENSE).
