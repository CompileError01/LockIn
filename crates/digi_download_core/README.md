# digi_download_core (patched copy)

This is a copy of [digiDownload](https://github.com/digiDownload/digiDownload) at commit `b3791215`,
under its MIT licence (see `LICENSE`).

LockIn's changes, because digi4school became a JavaScript app and the old HTML pages are gone:

- The book list now comes from the site's JSON API, `br/xhr/v2/synch`.
- Books open by code, at `https://digi4school.at/ebook/<code>`.
- Volume links are resolved against the address the book actually opened at.
- Added `Session::debug_report`, which describes the site's responses with no personal data.
- Removed the debugging proxy feature.
