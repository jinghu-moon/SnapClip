# 面板图标（Tabler）

这 7 个图标都是 24×24 的描边文件，对应面板上的 7 个图形：

| 文件 | 用在哪 | 来源 |
| --- | --- | --- |
| `swap.svg` | 尺寸卡：互换宽高 | Tabler |
| `center.svg` | 位置卡：居中到屏幕 | Tabler |
| `origin.svg` | 位置卡：移到原点 | Tabler |
| `close.svg` | 面板右上角关闭 | Tabler |
| `chevron.svg` | 预设下拉的箭头（展开时上下翻转） | Tabler |
| `check.svg` | 下拉里选中那一项的勾 | Tabler |
| `link.svg` | 保存面板：尺寸行两格之间的锁定 | 设计稿自带的 `ICON.link` |

`swap`…`check` 是 [Tabler Icons](https://tabler.io/icons)（MIT）的原文件。`link.svg` 不是：设计稿的
`docs/design/save-panel-demo-v3.html:709` 自己画了这个图标（16 单位、三条子路径），照它重画成 24 单位
才**和设计一致**，而换一个"看起来差不多"的 Tabler 图标则不是同一个图形 —— 所以这里用的是设计稿的路径，
只加了一层 `scale(1.5)`（见文件内注释），**没有**改任何坐标数字。

面板上画出来的就是这些文件本身，**不是照着它们重画的线段**。

## 管线：SVG → PNG（resvg）→ Direct2D

```
assets/icon/*.svg ──(cargo run -p icon-bake, resvg)──▶ assets/icon/*.png
                                                          │ include_bytes!
                                                          ▼
                              d2d/panel_icon.rs: WIC 解码 → ID2D1Bitmap（白、透明、96×96）
                                                          │ CLSID_D2D1ColorMatrix 按状态色着色
                                                          ▼
                                            ID2D1Bitmap1（预着色，96×96）
                                                          │ ID2D1DeviceContext::DrawBitmap
                                                          ▼
                                              面板上的 14 dip 图形（目标矩形负责缩放）
```

- **为什么烘而不是运行时解析**：Direct2D 自带的 SVG（`DrawSvgDocument`）在这台机器上一个像素都不画
  （实测记录见下），而"用线段重画"永远达不到"和设计稿一样"。所以让**真正的 SVG 渲染器**（resvg）
  出图，浮层只负责贴图与着色。
- **`resvg` 只出现在 `tools/icon-bake`**：它是开发期工具。浮层用的是烘好的 PNG，所以 capture crate
  与 app 都不引入 SVG 依赖。
- **96×96**：24 单位 × 4，覆盖到 400% 缩放；面板按 14 dip 往下缩，这一步是超采样下采样。
- **白色 + 透明底**：颜色留到绘制时给，所以一张 PNG 服务所有状态。
- **改动 SVG 之后必须重烘**：`cargo run -p icon-bake`。忘了也没关系 —— `cargo test -p icon-bake`
  会重新渲染并逐字节比对，不一致就红。

## 换图标 / 加图标时

1. 把 SVG 放进 `assets/icon/`（`viewBox="0 0 24 24"`；`currentColor` 由烘焙工具换成 `#ffffff`）。
   工具的缩放是 `96 / 24`（`tools/icon-bake/src/main.rs:26`），**不看文件自己的 viewBox**，所以小于 24 单位
   的画法要么按比例改坐标、要么用 `<g transform="scale(...)">` 包一层（`link.svg` 用的是后者）。
2. 跑 `cargo run -p icon-bake`。
3. 在 `crates/snapclip-ui/src/panel/icons.rs` 的 `Glyph` 里加一个变体（`ICON_COUNT` / `ALL` / `name()` /
   `png()` 各一处），再在调用点用 `Glyph::新名字`。
4. 因为 resvg 渲染，**不再有"Direct2D 的 SVG 子集"这类限制**（`<style>`、`<use>`、渐变、clip 都能画）。
   测试会盯住"每个变体指向不同文件""PNG 是 96×96 RGBA""PNG 与 SVG 一致"。

## 改完跑一次这两条

```
cargo test -p icon-bake                                              # PNG 是否跟 SVG 一致
cargo test -p snapclip-capture --lib -- --ignored --test-threads=1 the_panel_glyphs the_glyph_tint
```

第二条在真 GPU 上画面板，按"每个图形自己方框里的墨"断言：图形落进自己的格子、墨量是线稿而不是
一整块、并且**换一种状态色画出来的像素必须不同**（颜色矩阵没生效就会红）。

---

# 历史：为什么不再走 Direct2D 的原生 SVG

下面这段是 2026-05 的实测记录，保留下来是因为它解释了这条管线为什么长这样。
**不要再走 `DrawSvgDocument`**。

已确认可用的部分：

- `ID2D1DeviceContext5::CreateSvgDocument` 存在，8 个文件全部解析成功
  （`CreateStreamOnHGlobal` → `IStream` → `CreateSvgDocument`）。
- `ID2D1SvgDocument::GetRoot` + `ID2D1SvgElement::SetAttributeValue2(POD_TYPE_COLOR)` 可用。
- 一个坑：把 `SetTransform` 打在 `ID2D1DeviceContext5` 上、或复位时传空指针，**会直接
  ACCESS_VIOLATION（0xc0000005）**，在应用里表现为"点尺寸栏就闪退"。

当时卡住的地方：

- **`DrawSvgDocument` 一个像素都不画。** 不是位置错，是完全不出现。排除过：变换（有/无/仅平移/
  平移+缩放）、`SetViewportSize`、纯白不透明颜色、目标位图与文档是否同一 device，**连手写的
  `<svg viewBox="0 0 24 24"><rect fill="#ffffff"/></svg>` 也不画**。解析、着色、变换都对，
  只有最后一步是空的。
- 一次**误诊**也记下来：这批文件把 `fill`/`stroke` 写在根 `<svg>` 上靠继承，我一度认定"Direct2D 不做
  根属性继承"是根因，写了"把根画法解析出来显式盖到每个形状"（还要避开 Tabler 那个透明底框）。
  手写 SVG 用显式 `fill` 同样不画 ⇒ 方向错了，那次改动已撤回。

结论：原生 SVG 这条路在这台机器上不成立，而"线段重画"达不到设计稿的还原度 —— 于是改成
**让 resvg 出图、浮层贴图**，也就是上面那套管线。
