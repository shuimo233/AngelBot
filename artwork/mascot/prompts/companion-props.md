# 多道具素材与最终提示词

全部使用内置 **image_gen**，未使用 CLI/API 后备路径，未对 PNG 做程序化修图。
两个切件复用此前已确认的 AngelBot 形象、衣袖与分层骨架，原始生成文件保留在 Codex generated_images 中。

- [gamepad-back-hands.png](../companion/gamepad-back-hands.png)：最终使用的背壳朝镜头手柄，1717×916，985613 字节。原始生成名 `exec-1c47208f-1d49-44e5-a6bc-1871f1795d44.png`。
- [tea-saucer-hands.png](../companion/tea-saucer-hands.png)：一手托茶碟、一手握杯柄，1810×869，920790 字节。原始生成名 `exec-4e6ae74a-2e73-4b89-afb7-8e14c0d8e6fa.png`。
- [reading-parts.png](../companion/reading-parts.png)：保留此前的角色分件与捧书姿势，不是本次重新生成。

失败的按键面朝外手柄稿未接入。最终渲染应看到手柄背壳，拇指与按键面在靠近人物的一侧。
之前的双手捧杯稿 `mug-hands.png` 仅是当时的编辑基底，不随当前素材集提供；下文保留这段历史编辑说明。

## 手柄最终生成提示词

参考图：`reading-parts.png`，仅用于角色衣袖与绘制风格；`transparent_background: true`。

```text
Use case: stylized-concept. A technically correct transparent 2D cutout asset for an anime desktop girl PLAYING a game while facing the viewer. Show ONLY her two forearms, hands and a normal console gamepad. Match the reference girl's ivory flared/puffed sleeve cuffs with navy bows, small SD hands, clean dark anime ink contours and restrained lavender cel shading. Do NOT draw the head, torso, shoulder capes, legs, table or full girl.

CRITICAL CAMERA AND OBJECT ORIENTATION: The camera is IN FRONT OF the girl, at her chest level, looking straight at her. It is NOT a first-person view and NOT an over-the-player's-shoulder view. Her gamepad's button surface faces AWAY FROM THE CAMERA, TOWARD HER BODY and slightly upward toward her eyes. Therefore the CAMERA SEES THE SMOOTH BACK/UNDERSIDE SHELL of the controller, NOT its button face. The visible main shell has only normal back-casing seams, shallow grips and a subtle central panel. NO VISIBLE D-PAD, NO VISIBLE FACE BUTTON CLUSTER, NO VISIBLE ANALOG STICKS on the big camera-facing surface. This is not a diagram showing off all of the controls. The controls exist on the far side but are hidden by the back casing and hands. Do not turn the gamepad toward the audience to show its buttons.

CORRECT PLAYING GRIP: Two hands approach from the sleeve cuffs at left and right. Palms wrap snugly around the ergonomic outer handles. The middle/ring/little fingers curl around the handles onto the BACK SIDE that we can see; only sensible short curled fingers over the grip corners, not flat spread palms. The thumbs reach over onto the hidden upper/front control side facing the GIRL; from the camera only a small natural side view of the thumbs may peek above the top edge. Index fingers curve over the shoulder triggers along the top edge. The near back casing remains visible in the center, with fingers gripping at both sides. Natural wrists attached to sleeves, exactly two hands, no floating fingers or extra fingers.

COMPOSITION: one wide horizontal connected assembly on transparent background, both outer elbow/cuff attachment ends at equal height near the upper left/right, bent forearms leading inward and down to the gamepad held at waist/lower-chest level. Generous empty padding so all sleeves and fingers fit fully inside. Gamepad comfortably proportioned to two small hands, dark navy/slate back shell with a tiny unbranded gold accent, no logos or text. Professional Japanese anime SD cutout matching the reference style. Full true transparency outside the artwork, opaque interiors, clean alpha edges, no glow, no matte, no external shadow. This asset is specifically a FRONT VIEW OF A PERSON GAMING: the BACK of the controller faces us; the CONTROL FACE faces her and is NOT shown.
```

## 茶杯与茶碟最终编辑提示词

编辑目标：此前双手捧杯稿 `mug-hands.png`；`transparent_background: true`。只改变握持姿势并增加茶碟，保留衣袖、杯子设计及外侧接缝位置。

```text
Use case: precise-object-edit. Edit this existing transparent anime forearms-and-teacup rig asset. Preserve the exact ivory/navy sleeve design, navy bows, dark clean ink lines, pale skin tones, cel shading, cup decoration, overall character scale and the two outer elbow/cuff attachment positions. Change ONLY the tea-holding gesture and add one matching saucer.

NEW NATURAL GESTURE: The hand on the LEFT SIDE OF THE IMAGE supports a small matching ceramic SAUCER FROM UNDERNEATH, palm facing upward, fingers gently curved under the saucer's base and near rim, thumb resting lightly by the edge. This hand must not grip the cup body. The hand on the RIGHT SIDE OF THE IMAGE holds the cup BY ITS RIGHT-SIDE HANDLE, with the index finger naturally through the handle opening, thumb resting near the top of the handle, remaining fingers softly curled outside/below. This hand must not wrap around the body of the cup. Exactly TWO hands and TWO forearms, each connected to its sleeve, no extra fingers, no detached limbs.

The cup stands securely ON THE SAUCER, centered and touching its shallow center. Cup and saucer are level, not spilling or floating. The supporting palm should visibly carry the saucer's weight; don't pinch a plate vertically. Comfortable, relaxed asymmetry: lower supporting hand under the saucer, higher handle hand beside the cup, both wrists natural. Keep the right handle opening and finger placement readable. The mug must remain comfortably small compared with her hands, not become a bucket; saucer width about one and a half times cup diameter. Add an understated navy/gold matching rim to the ivory saucer, no extra decorative objects.

Preserve a single wide horizontal isolated assembly suitable for replacing the previous part in a layered desktop-pet rig. Both elbow attachment ends stay at the upper outside left/right positions; hands lead inward/down toward a cup and saucer near the lower center. Preserve canvas proportions around 1810x869 with generous transparent padding and no cropped sleeve or plate. NO head, NO torso, NO upper arms, NO legs, NO table, NO other props, NO steam (application adds it), NO text, NO labels. True transparent background, opaque painted interiors, clean antialiased cutout edges, no white or colored matte, glow, or external cast shadow. Japanese anime SD style exactly matching the existing artwork.
```
