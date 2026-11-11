#!/usr/bin/env python3
"""Generate deterministic fixture pages for the browser-driver ladder.

Two self-contained pages (no external resources — no network weather):
  page_a.html — product listing: 500 `.card` elements, a search box + button
                that client-side filters cards by title substring, and an
                order form (#cust, #qty, submit #place) whose submit a script
                handles (no navigation; #status records the order). The ops
                stage times a click on that submit and a 4-step form batch.
  page_b.html — detail page: heading + a 1000-row `#rows` table.

Seeded RNG => byte-identical fixtures on every machine.
"""
import json
import random
import sys
from pathlib import Path

SEED = 42
N_CARDS = 500
N_ROWS = 1000
VENDORS = ["acme", "oem-plus", "parts-yard", "rockpile", "mega-auto"]

OUT = Path(__file__).resolve().parent / "fixtures"


def gen_page_a() -> str:
    rng = random.Random(SEED)
    cards = []
    for i in range(N_CARDS):
        kind = "Widget" if i % 2 == 0 else "Gadget"
        suffix = "Pro" if i % 3 else "Max"
        title = f"{kind} {1000 + i} {suffix}"
        price = f"${rng.uniform(5, 500):.2f}"
        vendor = rng.choice(VENDORS)
        cards.append(
            f'<article class="card" data-vendor="{vendor}">'
            f'<h2 class="t">{title}</h2>'
            f'<span class="p">{price}</span></article>'
        )
    body = "\n".join(cards)
    return f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>Parts listing — 500 results</title>
<style>.card{{border:1px solid #ccc;margin:4px;padding:6px}}.hidden{{display:none}}</style>
</head><body>
<h1>Search results</h1>
<input id="q" type="text" value="">
<button id="search">Search</button>
<form id="order">
<input id="cust" type="text" value="" placeholder="Name">
<input id="qty" type="text" value="" placeholder="Qty">
<button id="place" type="submit">Place order</button>
<p id="status"></p>
</form>
<div id="list">
{body}
</div>
<script>
document.getElementById('search').addEventListener('click', () => {{
  const q = document.getElementById('q').value.toLowerCase();
  for (const c of document.querySelectorAll('.card')) {{
    const t = c.querySelector('.t').textContent.toLowerCase();
    c.classList.toggle('hidden', !t.includes(q));
  }}
}});
// The order form's submit is handled by a script: no navigation, the
// status line records the order. The ops stage clicks #place to time a
// click on a submit a script handles, and fills #cust/#qty + clicks
// #place + reads #status as its 4-step form batch.
document.getElementById('order').addEventListener('submit', (e) => {{
  e.preventDefault();
  const cust = document.getElementById('cust').value;
  const qty = document.getElementById('qty').value;
  document.getElementById('status').textContent = 'placed: ' + cust + ' x ' + qty;
  window.__placed = (window.__placed || 0) + 1;
}});
</script>
</body></html>
"""


def gen_page_b() -> str:
    rng = random.Random(SEED + 1)
    rows = []
    for i in range(N_ROWS):
        sku = f"SKU-{rng.randint(10000, 99999)}"
        name = f"Part assembly {i}"
        price = f"${rng.uniform(1, 999):.2f}"
        rows.append(f"<tr><td>{sku}</td><td>{name}</td><td>{price}</td></tr>")
    body = "\n".join(rows)
    return f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>Part detail — assembly diagram</title>
</head><body>
<h1 id="heading">Right rear door assembly</h1>
<table id="rows"><tbody>
{body}
</tbody></table>
</body></html>
"""


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "page_a.html").write_text(gen_page_a(), encoding="utf-8")
    (OUT / "page_b.html").write_text(gen_page_b(), encoding="utf-8")
    # Expected values the ladder asserts (computed from the same seed).
    expected = {
        "title_a": "Parts listing — 500 results",
        "cards_a": N_CARDS,
        "visible_after_filter_widget": N_CARDS // 2,  # every other card is a Widget
        "title_b": "Part detail — assembly diagram",
        "rows_b": N_ROWS,
        # The ops stage's 4-step form batch fills #cust "ada" and #qty "2",
        # clicks #place (submit, handled by the page script) and reads #status.
        "status_after_batch": "placed: ada x 2",
        "heading_a": "Search results",
    }
    (OUT / "expected.json").write_text(json.dumps(expected, indent=2), encoding="utf-8")
    for p in ("page_a.html", "page_b.html"):
        print(f"wrote {OUT / p} ({(OUT / p).stat().st_size} bytes)")


if __name__ == "__main__":
    sys.exit(main())
