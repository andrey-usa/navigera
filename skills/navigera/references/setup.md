# Setup details

Windows (PowerShell): `irm https://raw.githubusercontent.com/andrey-usa/navigera/master/install.ps1 | iex`;
single-quote JavaScript that contains `$` there. JavaScript with quotes,
backslashes or regex survives no shell's quoting reliably: save it to a file
and run `eval --file extract.js` (or pipe it to `eval -`). Chrome is found on
its own (a system install, or the chrome-headless-shell `navigera
install-browser` downloads from Google); else pass `start --chromium <path>`
or set `$CHROME_BIN`.
