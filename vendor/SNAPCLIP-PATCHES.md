# HDR AVIF 的固定源码补丁

本目录仅为 `codec-bench/hdr-avif-probe` 的显式资格验证服务，App／Pin／MIT Yue 不新增这些默认 codec 依赖。没有修改全局 Cargo registry。上游原包版本、SHA-256、许可证声明，以及本地补丁 SHA-256／修改文件清单在 `tools/codec-bench/hdr-avif-sources.json`。

## zenavif 0.1.6

- 原包 66 份文件保持可追溯，省略的仅是上游独立 `Cargo.lock`，workspace 使用根 lock；原 `LICENSE-AGPL3` 和 `LICENSE-COMMERCIAL` 都保留。
- `src/convert.rs`：有界、可取消、尊重 stride 的 GBR→RGB 复制与 RGB full/limited 范围处理。
- `src/decoder_managed.rs`：8/10/12 位 Identity 不走 BT.601；8 位 strip 入口同样转到正确路径。
- 同文件新增 `decode_grid_tiles`：解码前校验块尺寸／位深，每次只交付一块，合并对应 Alpha，裁掉边缘 padding；整帧便利 API 复用此路径。grid 的 10 位／4:4:4 信息来自实际序列头，不回退成 8 位／4:2:0。
- `new_with_stop` 将取消从构造入口传给 parser 和 grid 索引；单次压缩输入复制仍是同步操作，调用方必须限制输入大小。
- `src/config.rs`：显式 `grid_tile_limit`，默认仍保留 parser 的 1000 块；开发探针为 128×128 的大图实验独立设置 4096 块。像素和内存门槛没有提高。
- `src/encoder.rs`：RGB16／RGBA16 写入正确的 G/B/R 平面，使用迭代器去掉全图中间副本。
- `src/yuv_convert.rs`：将锁定依赖已弃用的 `splat/zero/from_array` 改为其对应 `_t` 名称，未更换 SIMD 算法或加 allow。

## zenavif-parse 0.6.2

- 原包 14 份文件，原 MPL-2.0 `LICENSE` 保留，仅省略其独立 `Cargo.lock`。
- 仅 `src/lib.rs` 改动：primary item 同时关联 ICC+nclx 时优先保留 ICC，与 association 顺序无关；不修改样本 matrix/range。
- 从实际 item payload 读取 grid 描述，不再通过 ispe 猜网格；校验描述长度、覆盖、ispe、dimg 数量与块类型。独立解析 Alpha grid，以 `alpha_tile_data` 暴露相同布局的块；不支持的异构布局明确拒绝。行列使用 u16 表达规范中的 256，当前编码器接口仍显式限制 1～255。

## zenavif-serialize 0.1.4

- 原包 11 份文件和 BSD-3-Clause `LICENSE` 保留，仅省略独立 `Cargo.lock`。仅 `src/grid.rs` 修改。
- grid 支持主图 ICC 和 nclx 同时关联；按同一来源／类型合并 dimg，保留行优先顺序。
- 修复扫描整份文件寻找 `BAAD_F00D` 偏移占位符的实现，只更新已记录的 iloc 字段，避免破坏 ICC 或压缩数据；测试覆盖同字节序列出现在两种 payload 中。
- 校验几何、Alpha 配置、item ID／32 位文件长度溢出，按准确总长度 reserve。此 serializer 仍返回完整压缩 Vec；探针有独立的 4 MiB 压缩数据／5 MiB 文件上限，不能称为任意大文件流式 muxer。

## 可复现验证

`python tools/check-hdr-codec-vendor.py` 将固定 `.crate` 的 SHA-256 与原清单比较，再逐文件与 vendored 源比较并重建 unified diff；差异必须与各目录的 `SNAPCLIP.patch` 逐字节相同。缺包时显式 `--download` 从 crates.io 拉取固定版本并检查 hash，不追踪 latest。

`.gitattributes` 对 `/vendor/**` 禁用自动换行转换，保证 Windows clone 与原包字节一致。修改或升级时必须重新审阅补丁与许可证、更新清单，重跑 `tools/test-hdr-avif.ps1`、普通工具测试和 workspace 门禁，不可仅修改期望 hash 当作验证。

第一方文件执行常规 fmt／`git diff --check`。原发布包 LICENSE／两个 examples 自带的行尾空白，以及 unified diff 必须带空格的空白 context 行，逐项确认后保留其原字节；没有为它们新增 whitespace allow 或全局降低门禁。第三方包使用原档案＋补丁逐字节检查，不对完整上游源进行格式清洗。

本补丁限定 managed decoder 和静态高位深编码的已测路径；没有为 `unsafe-asm`、zencodec、动画、异构 Alpha grid 或任意敌意元数据提供额外合格声明。来源校验不等于发布时的许可交付已经完成。整帧资源仍失败；有界 grid 的成功与超限拒绝测量见 `docs/53-hdr-capture.md` §27，不能把资格探针通过解释为产品已支持 AVIF。
