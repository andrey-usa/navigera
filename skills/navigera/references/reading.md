# Reading a page: `ax`

```text
page "Checkout — Acme" http://shop.test/checkout
- banner:
  - link "Cart 2" [ref=9] url=/cart
- main:
  - heading "Checkout" [level=1]
  - textbox "Full name" [ref=21]
  - combobox "Country" [ref=22] value="Select…" options: "Canada", "Mexico", …
  - group "Shipping speed":
    - radio "Standard (5–8 days)" [ref=24]
    - radio "Express (1–2 days)" [checked, ref=25]
  - Iframe "Card payment":
    - document "Card details":
      - textbox "Card number" [ref=41]
  - button "Place order" [ref=30]
```

- `[ref=N]` marks what you can act on: pass the number to `click`/`fill`/
  `select`/… (`@12`, `e12` and `ref=12` work too). A ref whose element the
  page drew again (same role, name and place) still works, and the result's
  note names the new ref; after a navigation, or when the page changed more
  than that, the ref is stale: take a fresh `ax`.
- `clickable [ref=N]: <text>` is something the page made clickable without
  saying so (a row, a card, an icon): click it by ref like a button, before
  reaching for `eval`. One with no text shows its `#id` or `.class` and
  what the page marks it with (`icon="fa-trash"`, `data-tooltip="Archive"`,
  `img="home.png"`).
- A cut text ends `… (+N chars: get text <ref>)`: that command reads it all.
  A snapshot that cut anything says so on its second line. Before a reading
  or compliance judgment (terms, policies, contracts), read in full: `text`
  (the whole page as it reads), `text --selector main`, or `ax --full-text`.
- Nesting shows what belongs together (which "Add to cart" sits in which
  product). Iframes and open shadow DOM are included; links show their `url=`.
- Collapsed or hidden content (closed `<details>`, hover menus, inactive
  tabs) is **not** in the tree until you open it: click the toggle or
  `hover` the menu, then `ax` again.
- Big page: `ax --selector "#results"` or `ax 57` (a ref), `--limit <lines>`
  (default 2000).
- One fact from a long page (a default, a price, a date): `text --grep
  "<word or phrase>"` prints the passages that mention it with the lines
  around them (`--context <lines>`, `--limit <passages>`), a few hundred
  bytes instead of the page. Several words match in any order when the
  phrase is nowhere.
- Many items: one `eval` that returns JSON:
  `navigera -s work eval "() => [...document.querySelectorAll('.item')].map(e => e.innerText)"`
