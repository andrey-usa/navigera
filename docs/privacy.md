# Privacy policy

Last updated: 17 November 2026.

This policy covers navigera: the command-line tool, its MCP server, and the
navigera plugin for Claude Code (the skill in this repository). It is
published by the navigera maintainers, the people who publish navigera at
[github.com/andrey-usa/navigera](https://github.com/andrey-usa/navigera).

## In short

navigera collects nothing. It has no account, no telemetry, no analytics
and no server of its own, and nothing you do with it is sent to its
maintainers.

## What stays on your machine

navigera runs on your computer. What it handles stays there: the pages it
opens and what it reads from them, cookies and site storage, screenshots,
PDFs, downloads, saved session state, recordings and logs. It keeps them in
files private to your user account until you, or navigera when a session
ends, delete them. With `--profile` or `--attach` it works in a browser
profile of yours, and changes only what you tell it to.

## What leaves your machine, and to whom

Only what you, or the agent you run, direct:

- **The websites you visit** through it receive what any browser sends them.
- **The browser itself** (Chrome, Edge, Firefox, Safari) makes its own
  requests, such as updates and safe-browsing checks, under its maker's
  policy. navigera turns several of Chrome's start-up requests off, not all
  of them (see the [changelog](../CHANGELOG.md)).
- **Downloads you start:** `navigera install-browser` fetches Chrome for
  Testing from Google; the installers and the plugin's first-run step fetch
  navigera's release from GitHub, or from npm, PyPI, Homebrew or Scoop if
  you install from there.
- **A remote browser or WebDriver server** you name with `--attach` or
  `--webdriver` (a cloud browser service, a Selenium grid) receives the
  session's traffic under its own terms.

## The AI model you use

When an agent such as Claude Code uses navigera, what navigera prints (page
text, the accessibility tree, screenshots) becomes part of the agent's
conversation and goes to its model provider, under that provider's policy.
navigera does not control that. Do not point it at pages whose content you
may not share with your provider.

## Credentials

navigera takes credentials from environment variables or files, never from
its command line, and masks them in what it prints and writes (see
[Security](../SECURITY.md)).

## Children

navigera is a tool for developers and is not meant for children. It
collects no one's data.

## Changes and contact

This policy changes only in this file, and its history is the repository's.
Questions: [open an issue](https://github.com/andrey-usa/navigera/issues).
Security reports: [SECURITY.md](../SECURITY.md) (private reporting).
