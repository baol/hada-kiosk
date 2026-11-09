# Licences

hada is free software, licensed under the GNU General Public License v3.0
only (GPL-3.0-only); see `LICENSE`.

It is built with the following third-party components:

| Component | Licence | Notes |
|---|---|---|
| [Slint](https://slint.dev) UI toolkit | GPL-3.0-only (one of Slint's three licence options) | hada uses Slint under the GPL, which is why hada itself is GPL-3.0. Slint's royalty-free licence does not cover embedded systems such as a dedicated kiosk. |
| [Material Design Icons](https://pictogrammers.com/library/mdi/) font (`@mdi/font` 7.4.47) | Apache-2.0 | `crates/hada/assets/materialdesignicons-webfont.ttf`, `mdi_variables.scss`; see `crates/hada/assets/MDI-LICENSE`. |
| Rust crates from crates.io | MIT / Apache-2.0 / similar, as declared by each crate | See `Cargo.lock`. |

Home Assistant's websocket protocol and Lovelace card schemas were implemented
from its documentation and source (Apache-2.0); no Home Assistant code is
included. `tests/fixtures/icons.json` and `tests/fixtures/translations_*.json`
contain icon names and state translations copied from Home Assistant's
integration files (Apache-2.0).
