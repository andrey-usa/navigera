# When something fails

| error | do this |
|---|---|
| `no element matches selector … / no element with text …` | `ax` to see what is really there; act by ref |
| `no element has that ref now` | take a fresh `ax` |
| `timed out … waiting for load` | a slow resource: `goto <url> --wait domcontentloaded`, then `wait --text …` |
| `select: element is … not a <select>` | `click` the dropdown, `ax`, then click the option by ref |
| `no navigera session at …` | `navigera -s <name> start`; the error says why the last one stopped |
| text you expect is missing from `ax` | collapsed, or further down: expand it or `scroll`, then `ax` again |

## Output and notes

- Output: compact, as above; errors go to stderr as `error: …` with exit
  code 1, notes as `note: …` (a dialog answered, a new tab, an exception the
  page threw: read them). `--output json` prints one JSON line per command,
  `{"ok":true,"result":…}` or `{"ok":false,"error":"…"}`, actions with
  `{tab, tabs, url, title}`, for programs.
