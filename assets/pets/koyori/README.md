# Koyori Pet skin

This is a normalized, embedded presentation asset derived from the project
owner's custom Codex Pet v2 skin. The source Codex manifest and WebP sheet are
not runtime inputs.

`atlas.png` is an 8-column by 4-row RGBA atlas. Each cell is 144 by 156 pixels.
Rows are `Idle` (6 frames), `MoveRight` (8), `MoveLeft` (8), and `Attend` (6).
Unused cells are transparent. The shared Slint surface displays a cell at 192
by 208 pixels.
