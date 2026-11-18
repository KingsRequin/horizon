# Third-party notices

Horizon is released under the MIT license (`LICENSE`). It bundles or adapts the following
works, under their own licenses.

| Work | Where | License |
|---|---|---|
| [Phosphor Icons](https://github.com/phosphor-icons/core) | `natif/icons/phosphor/` | MIT — `natif/icons/phosphor/LICENSE` |
| [Simple Icons](https://github.com/simple-icons/simple-icons) | `natif/icons/simple-icons/` | CC0 1.0 — `natif/icons/simple-icons/LICENSE.md` |
| [Inter](https://github.com/rsms/inter) | `natif/fonts/Inter-*.ttf` | SIL OFL 1.1 — `natif/fonts/OFL-Inter.txt` |
| [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) | `natif/fonts/JetBrainsMono-*.ttf` | SIL OFL 1.1 — `natif/fonts/OFL-JetBrainsMono.txt` |
| [Space Grotesk](https://github.com/floriankarsten/space-grotesk) | `natif/fonts/SpaceGrotesk-*.ttf` | SIL OFL 1.1 — `natif/fonts/OFL-SpaceGrotesk.txt` |
| [Halo Battery](https://github.com/HeyOkay/HaloBattery) | headset protocols, `crates/horizon-core/src/headset/` | MIT (notice below) |

Simple Icons are brand logos: the brands remain the property of their owners; their use here
only identifies the matching app or website.

The headset battery protocols were ported from Halo Battery's Python providers. Halo Battery
documents them from public sources (HeadsetControl, Solaar, OpenRazer, Linux-Arctis-Manager,
vendor documentation); each module of `crates/horizon-core/src/headset/` names its own sources.
No code from those projects is copied: only the protocol facts (identifiers, packet layouts)
needed to talk to the devices.

## Halo Battery

```
MIT License

Copyright (c) 2026 Halo Battery contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
