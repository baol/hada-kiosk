# Licences

## hada's own code

The source code in this repository is licensed under either of

- the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), or
- the MIT License ([LICENSE-MIT](LICENSE-MIT)),

at your option. Releases up to and including v0.5.0 were published under
GPL-3.0-only; those copies keep that licence.

Unless you explicitly state otherwise, any contribution you submit for inclusion
in hada shall be dual licensed as above, without any additional terms or conditions.

## Slint, and the licence of each build

hada draws its user interface with [Slint](https://slint.dev), which is offered
under three licences: GPL-3.0-only, the Slint Royalty-free Desktop, Mobile, and Web
Applications License 2.0, and a commercial licence. Each build picks one:

| Build | Slint licence | The build as a whole |
|---|---|---|
| Linux binaries and `.deb` packages (desktop window, Raspberry Pi kiosk) | GPL-3.0-only | GPL-3.0-only ([LICENSES/GPL-3.0-only.txt](LICENSES/GPL-3.0-only.txt)). The royalty-free licence does not cover embedded systems, such as a dedicated wall panel. |
| Android APK | GPL-3.0-only | GPL-3.0-only |
| A future iOS app | Royalty-free 2.0 | MIT OR Apache-2.0, plus Slint's terms: a Mobile Application with Slint attribution (the `AboutSlint` widget in the app, or the "Made with Slint" badge on the download page). |

Anyone building hada into their own product chooses the Slint licence that fits it.

## Other components

| Component | Licence | Notes |
|---|---|---|
| [Material Design Icons](https://pictogrammers.com/library/mdi/) font (`@mdi/font` 7.4.47) | Apache-2.0 | `crates/hada/assets/materialdesignicons-webfont.ttf`, `mdi_variables.scss`; see `crates/hada/assets/MDI-LICENSE`. |
| Rust crates from crates.io | MIT / Apache-2.0 / BSD / Zlib / similar, as declared by each crate | See `Cargo.lock`; Slint is the only copyleft dependency. |

Home Assistant's websocket protocol and Lovelace card schemas were implemented
from its documentation and source (Apache-2.0); no Home Assistant code is
included. `tests/fixtures/icons.json` and `tests/fixtures/translations_*.json`
contain icon names and state translations copied from Home Assistant's
integration files (Apache-2.0).
