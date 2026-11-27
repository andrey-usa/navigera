#!/usr/bin/env python3
"""Generate deterministic fixture pages for the browser-driver ladder.

Three self-contained pages (no external resources — no network weather):
  page_a.html — product listing: 500 `.card` elements, a search box + button
                that client-side filters cards by title substring, and an
                order form (#cust, #qty, submit #place) whose submit a script
                handles (no navigation; #status records the order). The ops
                stage times a click on that submit and a 4-step form batch.
  page_b.html — detail page: heading + a 1000-row `#rows` table.
  page_c.html — large listing: 3,000 product rows of 20 elements each
                (~60,000 elements) with 40 open shadow roots in a custom
                element an inline script defines, and a #more button whose
                handler (no navigation) marks one row and writes #status.
                The ops stage times a click on #more, so a tool's cost that
                grows with the page (a walk of every element, a whole-page
                query) shows against the others.

Seeded RNG => byte-identical fixtures on every machine.
"""
import json
import random
import sys
from pathlib import Path

SEED = 42
N_CARDS = 500
N_ROWS = 1000
N_ROWS_C = 3000
# One custom element with an open shadow root every 75 rows: 40 roots.
SHADOW_EVERY = 75
# The row #more marks.
MARK_ROW = 7
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


def gen_page_c() -> str:
    rng = random.Random(SEED + 2)
    rows = []
    for i in range(N_ROWS_C):
        sku = f"SKU-{rng.randint(10000, 99999)}"
        price = f"${rng.uniform(1, 999):.2f}"
        vendor = rng.choice(VENDORS)
        stock = rng.randint(0, 40)
        # 20 elements a row.
        rows.append(
            f'<div class="row" id="r{i}" data-vendor="{vendor}">'
            f'<span class="sku">{sku}</span>'
            f'<span class="n"><b>Part</b> <i>{i}</i></span>'
            f'<ul><li>steel</li><li>{vendor}</li><li>{stock} left</li><li>ships 2d</li></ul>'
            f'<p><a href="#r{i}">details</a> <em>{price}</em> <small>ex. VAT</small></p>'
            f'<div class="s"><span>A</span><span>B</span><span>C</span><span>D</span></div>'
            f'<strong>{stock}</strong></div>'
        )
        if i % SHADOW_EVERY == 0:
            rows.append("<x-card></x-card>")
    body = "\n".join(rows)
    return f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>Parts catalogue — {N_ROWS_C} rows</title>
<style>.row{{border-bottom:1px solid #eee;padding:2px}}.row.on{{background:#ffd}}</style>
</head><body>
<h1>Catalogue</h1>
<button id="more" type="button">More</button>
<p id="status"></p>
<div id="list">
{body}
</div>
<script>
// Each x-card holds an open shadow root, so a tool that looks into shadow
// roots on a click has some to find.
customElements.define('x-card', class extends HTMLElement {{
  constructor() {{
    super();
    this.attachShadow({{mode: 'open'}}).innerHTML =
      '<div><span>card</span><button type="button">Open</button></div>';
  }}
}});
// #more changes one row and the status line (no navigation); the ops stage
// clicks it and reads #status. The counter makes the expected status exact.
document.getElementById('more').addEventListener('click', () => {{
  document.getElementById('r{MARK_ROW}').classList.add('on');
  window.__more = (window.__more || 0) + 1;
  document.getElementById('status').textContent = 'more: ' + window.__more;
}});
</script>
</body></html>
"""


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "page_a.html").write_text(gen_page_a(), encoding="utf-8")
    (OUT / "page_b.html").write_text(gen_page_b(), encoding="utf-8")
    (OUT / "page_c.html").write_text(gen_page_c(), encoding="utf-8")
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
        "title_c": f"Parts catalogue — {N_ROWS_C} rows",
        # Elements in the document (getElementsByTagName('*')): html, head,
        # meta, title, style, body, h1, #more, #status, #list, script, the
        # rows' 20 each and the x-cards. Shadow content is not counted.
        "elements_c": 11 + 20 * N_ROWS_C + len(range(0, N_ROWS_C, SHADOW_EVERY)),
        "shadow_roots_c": len(range(0, N_ROWS_C, SHADOW_EVERY)),
        # One click on #more from a reset page (status '', counter 0).
        "status_after_more": "more: 1",
        "mark_row_c": f"r{MARK_ROW}",
    }
    (OUT / "expected.json").write_text(json.dumps(expected, indent=2), encoding="utf-8")
    for p in ("page_a.html", "page_b.html", "page_c.html"):
        print(f"wrote {OUT / p} ({(OUT / p).stat().st_size} bytes)")


if __name__ == "__main__":
    sys.exit(main())
