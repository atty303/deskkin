# Night garden artwork

Three original images generated with the built-in OpenAI image-generation tool
on 2026-09-04 for this Deskkin demo. No downloaded stock images or third-party
characters are used. These assets are distributed under the repository's MIT
license. The existing Koyori skin retains its separate provenance.

The checked-in `.rgba` files are the canonical, normalized sprite artwork:
96 x 96, row-major straight-alpha RGBA8, four bytes per pixel, no header.
They preserve transparent backgrounds and partially transparent edges. They
are final image assets, not build-generated source code. Each file is 36,864
bytes; compile-time array sizes check the format's byte length. No image tool
or network access is needed to build or run them.

The renderer converts each sprite once to RGB565+A8 in its owned heap (APPCPU
PSRAM on CoreS3): 82,944 bytes for all three. Repeated objects share textures.
The 9 x 9 light sprite is procedural and uses another 243 bytes. No intermediate
scaled image is allocated per frame. Scene placement, movement, IDs and the demo
capacity live in `crates/deskkin-presentation/src/demo_world.rs`.

## Generation prompts

Built-in generation, one image per prompt; transparent output was requested.
Final sprites were normalized to 96 x 96 RGBA8 with alpha-preserving resampling.

### Terrarium

Use case: stylized-concept. Asset type: transparent game sprite for a tiny
320x240 embedded screen, night floating garden. Subject: one charming round
glass terrarium hanging in midair, lush jade and sage fern leaves and a few tiny
warm amber flowers, small cream ceramic bottom with brass rim. Front-facing
orthographic illustration, entire isolated object centered, no floor, no ground
shadow, no scene, no rope outside frame. Hand-painted cozy storybook game art
with crisp chunky silhouette and simplified forms, legible when scaled down to
96x96 pixels. Restrained teal / sage / ivory / amber palette. Genuine transparent
background and clean alpha edges. No text, no logo, no watermark, no extra
objects. Square composition, generous transparent margin, object fills about
80% of frame.

### Drone

Use case: stylized-concept. Asset type: transparent game sprite for a tiny
320x240 embedded screen, night floating garden. Subject: one adorable tiny
hovering garden robot, ivory rounded shell, single wide dark teal visor with
two tiny warm amber eyes, short brass side fins, small green leaf sprout
antenna. No arms, no legs, no pedestal, no scene. Front-facing orthographic
illustration, entire isolated object centered. Cozy hand-painted storybook game
art, chunky clear silhouette and simple large shapes readable when downscaled
to 80x80. Restrained ivory / desaturated teal / warm brass palette, not glossy
photorealistic. Genuine transparent background, clean alpha edges. No floor
shadow, no text, no logo, no watermark, no surrounding elements. Square frame,
object fills 80%.

### Lantern

Use case: stylized-concept. Asset type: transparent game sprite for a tiny night
floating garden. One small glowing amber paper lantern, rounded egg-shaped
pleated cream paper shade, teal-green cap, tiny brass ring on top, short
red-copper tassel underneath. Hand-painted cozy storybook game illustration,
simplified chunky silhouette with broad colors legible at 64 pixels,
front-facing orthographic view. Warm amber light inside, restrained ivory sage
teal brass palette. Entire object centered inside square canvas with 15% empty
margin. Genuine transparent background. No chain, rope, floor, shadow,
background scenery, text, logos or other objects.

## Preview

`mise run simulator:world-preview -- OUTPUT.ppm [DRAG_PX] [TIME_MS]` exports a
320 x 240 fixture through the simulator's real Slint capture and integer world
renderer, without a display server, host identity, or network. The output must
not exist; failed writes are removed. Drag uses the same target/observed lag as
the demo. For example, 80 pixels with 1000 ms yields a quarter-turn view.
Unknown and Notice are synthetic fixture values, not live service status.
