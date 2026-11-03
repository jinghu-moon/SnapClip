# HDR AVIF 的固定源码补丁

本目录仅为 `codec-bench/hdr-avif-probe` 的显式资格验证服务，App／Pin／MIT Yue 不新增这些默认 codec 依赖。没有修改全局 Cargo registry。上游原包版本、SHA-256、许可证声明，以及本地补丁 SHA-256／修改文件清单在 `tools/codec-bench/hdr-avif-sources.json`。

## zenavif 0.1.6

- 原包 66 份文件保持可追溯，省略的仅是上游独立 `Cargo.lock`，workspace 使用根 lock；原 `LICENSE-AGPL3` 和 `LICENSE-COMMERCIAL` 都保留。
- `src/convert.rs`：有界、可取消、尊重 stride 的 GBR→RGB 复制与 RGB full/limited 范围处理。
- `src/decoder_managed.rs`：8/10/12 位 Identity 不走 BT.601；8 位 strip 入口同样转到正确路径。
- `src/encoder.rs`：RGB16／RGBA16 写入正确的 G/B/R 平面，使用迭代器去掉全图中间副本。
- `src/yuv_convert.rs`：将锁定依赖已弃用的 `splat/zero/from_array` 改为其对应 `_t` 名称，未更换 SIMD 算法或加 allow。

## zenavif-parse 0.6.2

- 原包 14 份文件，原 MPL-2.0 `LICENSE` 保留，仅省略其独立 `Cargo.lock`。
- 仅 `src/lib.rs` 改动：primary item 同时关联 ICC+nclx 时优先保留 ICC，与 association 顺序无关；不修改样本 matrix/range。

## 可复现验证

`python tools/check-hdr-codec-vendor.py` 将固定 `.crate` 的 SHA-256 与原清单比较，再逐文件与 vendored 源比较并重建 unified diff；差异必须与各目录的 `SNAPCLIP.patch` 逐字节相同。缺包时显式 `--download` 从 crates.io 拉取固定版本并检查 hash，不追踪 latest。

`.gitattributes` 对 `/vendor/**` 禁用自动换行转换，保证 Windows clone 与原包字节一致。修改或升级时必须重新审阅补丁与许可证、更新清单，重跑 `tools/test-hdr-avif.ps1`、普通工具测试和 workspace 门禁，不可仅修改期望 hash 当作验证。

第一方文件执行常规 fmt／`git diff --check`。原发布包 LICENSE／两个 examples 自带的行尾空白，以及 unified diff 必须带空格的空白 context 行，逐项确认后保留其原字节；没有为它们新增 whitespace allow 或全局降低门禁。第三方包使用原档案＋补丁逐字节检查，不对完整上游源进行格式清洗。

本补丁限定 managed decoder 和静态高位深编码的已测路径；没有为 `unsafe-asm`、zencodec、动画或任意畸形文件提供额外合格声明。来源校验不等于发布时的许可交付已经完成。资源门槛尚未通过，不能把这些文件出现在仓库中解释为产品已支持 AVIF。
