# Upstream Baseline and Fork Features Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Adopt the upstream master baseline, port three fork behaviors without losing upstream improvements, validate rimecut-public compatibility, then publish to fork master only with explicit history-rewrite authorization.

**Architecture:** Build a candidate from a pinned upstream commit in an isolated worktree. Keep DJI parsing/rig/timing from upstream, add only required consumer compatibility, and implement GoPro GPS normalization and optional DOP as independently tested changes. The approved contract is `dop: null` for missing values.

**Tech Stack:** Rust, Cargo, serde/serde_json, GPMF/KLV, DJI protobuf, Python bindings, wasm-bindgen.

---

设计依据：`docs/plans/2026-11-10-upstream-feature-port-design.md`。本文件是待执行计划，不代表命令已运行或测试已通过。不得向 upstream 推送。

## Task 1：锁定远端快照并保存原 master

**Files/refs:** Git refs；本轮两个 `docs/plans/2026-11-10-upstream-feature-port-*.md`。

1. 执行 `git status --short --branch`、`git fetch origin master`、`git fetch upstream master`、`git rev-parse origin/master upstream/master`，保存完整 SHA。检查原 master 是否仍为设计快照。
2. 对上游新增提交重新检查 DJI pano、GoPro GPS9 和 GpsData.dop；若已有同等实现，不重复移植。
3. 创建不覆盖现有 ref 的本地备份分支，例如 `git branch backup/fork-master-before-upstream-20261110 <OLD_SHA>`。执行远端备份前确认权限，推送同名备份分支并核对 SHA。
4. 确保文档已经保存到提交；不要把 `nul` 加入提交，不要对用户未跟踪文件执行清理。
5. 使用 using-git-worktrees 技能建立 `port/upstream-features` 集成分支及独立工作区，起点为固定 `<UPSTREAM_SHA>`；切换后将设计和计划带入候选历史。原 master 暂时不动。

**Expected:** 可追溯的旧 master、固定上游 SHA、干净候选工作区。没有 master reset 或强制推送。

## Task 2：建立纯上游验证基线

**Files:** 不修改产品源码；必要时记录工具链选择。

1. 记录 `rustc --version`、`cargo --version`、`rustup toolchain list`、`rustup target list --installed`。
2. 在原样 upstream 候选上运行 `cargo test --locked`、`cargo test --locked --features sony-xml`。
3. 若 rustc 1.93.1 对 Cooke if-let guard 报错，先选择可运行原上游的工具链，再重复检查。不修改上游源码以掩盖工具链问题；未经授权不安装/升级系统级工具。
4. 按后续 manifest 检查独立 bin crate 的环境基线，区分缺系统依赖与新增回归。

**Expected:** 已记录通过/失败的基线；工具链无法构建时停止后续发布，不伪造通过结果。

## Task 3：确认 rimecut-public 消费契约并适配 DJI

**Files:** `src/dji/mod.rs` 的 `handle_parsed!` / `handle_pano!`；同文件测试模块或新增 `tests/dji_consumer_compat.rs`（仅当公开 API 可测试）；消费仓库只读检查。

1. 取得 rimecut-public 路径或用户提供的具体读取代码，记录 commit、Cargo 依赖和 metadata 路径。当前工作区未发现该仓库，此步是兼容验收的必需输入。
2. 先写失败契约测试：带有已知 pano 参数的输入经过真实 metadata 输出路径后，消费端能读取；同时保留上游 StreamMetadata 和 CameraRig。测试参数相等，不用“tag 存在”替代消费结果。
3. 如果消费端依赖旧 `Default/Metadata.pano_dewarp_params`，在新版 clip JSON 中做兼容投影，参数源是已有 stream；保留原上游 stream tag、fps 和 rig 逻辑。若直接消费上游已可用，只增加回归测试。
4. 写缺少 stream/pano 的测试，并断言没有新增虚假标定参数；覆盖相关 DJI Schema，避免共享宏错误访问不具该字段的类型。
5. 运行 `cargo test --locked dji` 与完整库测试；真实样例验证 rig/timing 未改变。
6. 单独提交，例如 `Preserve DJI panorama metadata consumer compatibility`。

**Expected:** rimecut-public 既有接口可用、有实际消费证据。缺消费代码时标记阻塞，不假定某一路径已验证。

## Task 4：移植 GoPro GPS5/GPS9 规范化

**Files:** `src/gopro/klv.rs`、`src/gopro/mod.rs`；可新增 `src/gopro/gps.rs` 以隔离后处理和测试（避免无必要重构）。

1. 从旧 master 的 `61e76ea`、`e3f58d3` 提取行为需求，不覆盖整个文件。
2. 写失败测试：GPS9 映射为 GPS/Data；GPS5 i32 和 GPS9 i32/Scalar 转换为 Vec_GpsData；无 GPS 不改变数据。
3. 先跑 `cargo test --locked gopro` 确认新断言因缺少能力失败，而非测试编译环境失败。
4. 实现最小映射和后处理。在原样新版 `process_samples` 后处理中接入，不替换 imu_start_times_ms、stamp_us、采样持续时间逻辑或 u64 文件大小参数。
5. 增加真实字节级 KLV 测试：TYPE + GPS9 homogeneous/heterogeneous 单行、多行；避免只构造 TagMap 而遗漏生产路径。
6. 增加短行、未知类型、已转换输入、不同 SCAL、缺 GPSU/GPSF、未定位行测试。GPSF OR 回退需协议/样例支持；不能将缺失字段伪造成定位。
7. 验证速度单位：若按 GpsData 契约输出 km/h，则原始 5000 / SCAL 1000 的 5 m/s 应为 18 km/h；检查消费方是否做二次换算，记录旧 fork 差异。GPS9 epoch 示例应断言 `1699964800.0`，不要照搬旧测试错误日期注释。
8. 运行 `cargo test --locked gopro` 和完整库测试；上游 GoPro timing 测试必须保留并通过。
9. 单独提交，例如 `Add normalized GoPro GPS5 and GPS9 telemetry`。

## Task 5：增加 DOP 字段与稳定序列化契约

**Files:** `src/tags_impl.rs`、GPS 后处理模块、`src/camm/mod.rs`、`src/insta360/record.rs`、`src/gyroflow/binary.rs`、相关测试；检查新增 GpsData 初始化点。

1. 写失败 JSON 契约测试：默认 GpsData 序列化包含 `dop: null`；Some(1.0) 输出数值 1.0。不得使用 skip_serializing_if。
2. 新增 `pub dop: Option<f64>`，保留 Default。所有无可靠 DOP 来源的初始化点填 None。
3. GPS9 列 7 按 SCAL 解码；用 100/100 -> Some(1.0)、无效/零值 -> None、GPS5 -> None 固定行为。DOP 不能未经协议核实标成 HDOP。
4. 运行 `cargo test --locked`、`cargo test --locked --features sony-xml`。确认 struct 新字段的下游 Rust 源码适配，不宣称 Option 自动带来源码兼容。
5. 单独提交，例如 `Expose optional GPS dilution of precision`。

## Task 6：跨绑定与消费方集成验证

**Files:** 原则上只新增针对性测试；若改 WASM channels 逻辑，单独审查范围和输出契约。

1. 运行 `cargo check --locked --manifest-path bin/gopro-gps/Cargo.toml`。
2. 运行 `cargo check --locked --manifest-path bin/python-module/Cargo.toml`；可用时构建 Python 包并检查 GPS JSON 的 dop/null。
3. 运行 `cargo check --locked --manifest-path bin/wasm-module/Cargo.toml --target wasm32-unknown-unknown`；可用时在该目录运行 `wasm-pack build --release --target web`。
4. 验证 WASM `telemetry()`、`series()` 和 `telemetry_flat` 的 GPS 输出：有/无 dop 混合时字段顺序、通道数和每行数值对齐；不把 null 的省略/通道位移作为允许行为。
5. 检查独立工具：`cargo check --locked --manifest-path bin/gyro2bb/Cargo.toml`、`cargo check --locked --manifest-path bin/gyroflow_proto_convert/Cargo.toml`、`cargo check --locked --manifest-path bin/gyroflow_proto_inject/Cargo.toml`。最后一个需 native FFmpeg，失败原因独立记录。
6. 用实际 GPS5/GPS9、DJI 全景样例在 rimecut-public 做解析和导入验证，比较定位、速度、时间、DOP、标定参数和 rig。缺失样例/消费仓库时保留明确未验证项。
7. 检查 `git diff --check <UPSTREAM_SHA>..HEAD` 和候选相对上游 diff；不顺手清理上游已有格式问题。请求独立代码审查。

## Task 7：授权后更新 fork master 与本地 master

**Files/refs:** fork origin/master、本地 master、备份 ref；不得修改 upstream。

1. 汇报候选 SHA、备份 SHA、测试结果、未验证项、下游接口差异。重新 `git fetch origin master`，确认 old SHA 未改变。
2. 明确说明新基线不包含原 fork commits，更新远端需要历史改写；取得用户对该具体发布的授权，不把“确认设计”等同为强推授权。
3. 只在授权后执行精确 lease：`git push --force-with-lease=refs/heads/master:<OLD_SHA> origin <CANDIDATE_SHA>:refs/heads/master`。lease 失败立即停止。
4. 确保本地 master 无用户改动/其他工作区占用后，再将其指向候选。若需 reset，先明确展示目标并保留备份；不得盲目 reset --hard。
5. 验证 `git rev-parse master origin/master` 一致，`git merge-base --is-ancestor <UPSTREAM_SHA> master` 成功，`git rev-list --left-right --count master...origin/master` 为 0/0。
6. 保留旧 master 备份和候选分支，交付准确 SHA。回滚同样重新授权并使用精确 lease。
