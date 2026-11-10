# A first session

```sh
navigera -s work start                      # warm headless browser behind a local socket
navigera -s work goto example.com
navigera -s work --raw ax                   # page as an indented tree, [ref=N] on actionable nodes
navigera -s work click 12                   # act on a ref
navigera -s work fill 31 "hello" && navigera -s work press Enter
navigera -s work wait --text "Saved"
navigera -s work quit
```

```text
page "Checkout — Acme" http://shop.test/checkout
- main:
  - heading "Checkout" [level=1]
  - textbox "Full name" [ref=21]
  - combobox "Country" [ref=22] value="Select…" options: "Canada", "Mexico", …
  - radio "Express (1–2 days)" [checked, ref=25]
  - Iframe "Card payment":
    - document "Card details":
      - textbox "Card number" [ref=41]
  - button "Place order" [ref=30]
```

The commands are `goto`, `ax`, `click`, `fill`, `type`, `press`, `select`,
`hover`, `drag`, `scroll`, `upload`, `wait`, `eval`, `back`/`forward`/`reload`,
`screenshot`, `pdf`, the `tab-*` commands, `dialog` and `quit`, with `get`
and `is` for one value, `console` and `network` for what the page logged and
fetched, `cookies`, `storage` and `state save|load` (Playwright's
`storageState` format) for sign-ins, `set` for viewport, device, media, geo
and the like, and `route` to fail or answer requests.

- **Targets:** elements can be named by ref, `--selector <css>`,
  `--text "<visible text>"`, `--role/--name`, `--label`, `--placeholder` or
  `--testid`.
- **Output:** every command prints one JSON line, or just the result with
  `--raw`.
- **Help:** `navigera help <command>`.
- **Agent guide:** the version-matched guide is
  [`skills/navigera/SKILL.md`](../skills/navigera/SKILL.md),
  also printed by `navigera skill`.
- **Reference:** [docs/navigera.md](navigera.md) has the full
  reference, and [llms.txt](../llms.txt) is an index for LLMs.
