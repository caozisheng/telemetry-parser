# 主仓新基线与 fork feature 移植设计

状态：用户已确认设计；尚未实施。日期：2026-11-10。

## 目标与非目标

以 AdrianEddy/telemetry-parser 的 master 为新的代码基线，按功能而不是按旧提交冲突块，重新移植 fork 的三项新增行为。最终更新 caozisheng/telemetry-parser 的 master 和本地 master；不向 AdrianEddy 主仓提交 PR。

本轮只交付设计与实施计划，不重置分支、不推送、不修改产品代码。三项 feature 是逻辑拆分，不表示 Cargo feature flag。

## 已核实基线

- fork master / origin/master：`20a78c4580e66af5ab7b314c30ff57831b3c3c46`。
- 已抓取的 upstream/master：`9350bc2e4b57b8c83315c3bcb80ffa0ba91975d4`。
- 分叉点：`fd9a73e87db90a39df86332be2f95f4d50112d2a`；fork 独有 4 个提交，上游独有 37 个提交。
- 实施前重新 fetch，记录两端完整 SHA；若上游推进，重新检查三项能力，不将本文件中的快照称为实时最新版本。
- 当前存在未跟踪文件 `nul`，不纳入设计提交，也不删除或覆盖。

### 对此前判断的修正

此前聊天中“上游已实现 GoPro GPS5/GPS9 规范化”的结论不正确。对 `9350bc2` 的实际检查显示：

- `src/gopro/klv.rs` 只将 GPS5 映射为 GPS/Data，未映射 GPS9。
- `src/gopro/mod.rs` 不含 fork 的 `process_gps_samples`，没有将 GPS5/9 规范化为 Vec_GpsData。
- 通用 KLV 层已有 heterogeneous Scalar 解码，并不等价于已有 GPS9 规范化。
- 主仓 DJI 已保留 StreamMetadata、读取 pano 参数并生成 CameraRig，但没有 fork 的旧 Clip Metadata JSON 兼容路径。
- 主仓 GpsData 没有 dop 字段。

## 选择与取舍

选择：在独立集成分支/工作区从上游快照建立干净基线，按三项 feature 逐项适配和测试，验收后发布为 fork master。

不选择：直接合并旧 master 再逐块选择 ours/theirs。这样容易把新版时间处理、u64 接口与旧解析代码混用。不选择：原样 cherry-pick 全部旧提交；这些提交包含已过时的上下文以及需要重新评估的数据语义。

## Feature 1：DJI pano 消费接口兼容

### 保留上游的能力

保留 `src/dji/mod.rs` 中的 Schema 识别、handle_pano、CameraRig 推导、StreamMetadata、帧率和时间处理。上游 stream metadata 是参数来源，不重新实现镜头标定或 rig 推导。

### 兼容层

消费方 `rimecut-public` 必须可以直接消费升级后的输出。当前没有可访问的本地消费仓库，不能声称已验证其接口。

实施前取得实际消费代码、锁定其版本及读取路径。若其依赖 fork 现有 `Default/Metadata` JSON 内的 `pano_dewarp_params`，则保留这一字段作为兼容投影，同时继续输出上游 StreamMetadata 和 CameraRig。只投影存在的参数，不借此修改其他 metadata 或时间计算。

若消费方已经能直接读取上游 StreamMetadata/CameraRig，则优先沿用上游，只补契约回归测试，不为凑齐三项而重复增加字段。

### 验收

用相同参数验证上游来源、兼容路径与消费方解析结果相等；缺少 stream/pano 时不崩溃、不伪造参数；不改变上游 rig、帧率和 IMU 输出。消费方未提供时，本项可完成候选实现，但整体验收不能标记“rimecut-public 已兼容”。

## Feature 2：GoPro GPS5/GPS9 规范化

移植 fork `61e76ea` 与 `e3f58d3` 所表达的能力，而不是覆盖主仓整个 GoPro 文件。

- 在 `src/gopro/klv.rs` 中识别 GPS9 的 GPS/Data 映射。
- 在新版处理流程中加入 GPS 后处理，输出 `GroupId::GPS / TagId::Data` 的 Vec_GpsData。
- 支持 GPS5 homogeneous i32、GPS9 i32 与 heterogeneous Scalar 形态；检查单行/多行真实 KLV 分派，不能只测手造 tag map。
- 保留主仓 imu_start_times_ms、stamp_us、get_avg_sample_duration、Fusion/360 与 parse(size: u64) 接口和测试。
- 无 GPS、已规范化数据、短行或无效输入应安全处理，不把缺失值强制解释为有效定位。

### 迁移时必须验证的数据语义

旧 fork 的 GoPro speed 由 SCAL 解码后为 m/s，而 GpsData 和主仓 viewer 的规范化分支按 km/h 消费；不能静默搬运这个不一致。以实际消费接口和样例验证最终单位，必要时在明确边界转换，并记录相对旧 fork 的行为差异。

GPS9 时间采用 2000 年 epoch 加日期/日内时间，GPS5 使用 GPSU。保留无定位行，通过 is_acquired 标识。GPS9 行级 fix 与 frame-level GPSF 的优先级需结合协议/真实样例验证；旧 fork 的 OR 规则作为兼容候选，不作为未经验证的协议结论。GPS5 同包多点时间精度限制需明确，不在此次同步中凭空推断时间。

## Feature 3：DOP 字段与序列化

在 `src/tags_impl.rs::GpsData` 保留 `pub dop: Option<f64>`。按用户最终选择，字段名为 dop，None 序列化为 JSON null，不使用 skip_serializing_if。

GPS9 从列 7 按对应 SCAL 解码；零值/未知值策略由用例固定。GPS5、CAMM、Insta360、Gyroflow 等无来源值时设 None，不伪造精度。

名称维持 DOP，不在没有协议证据时将其宣称为水平 HDOP。上层若称 HDOP，需核实其含义。

增加 public struct 字段会影响上游外部 Rust struct literal，JSON null 不解决 Rust 源码兼容性；检查 rimecut-public 的编译和用法。检查 WASM 通过 JSON 推导 channels 时，null/数值混合是否保持通道对齐。

## Git 发布与回滚

1. 保存原 origin/master 的完整 SHA，创建本地与远端备份 ref；保存本轮文档，不能在切换基线时丢失。
2. 从固定 upstream SHA 建集成分支，逐 feature 形成独立、可测试提交，保留旧 master 不动直至验收。
3. 新历史不会包含旧 fork master，不能称为从旧 master 快进到上游。
4. 发布前重新 fetch origin，核对 master 未被他人更新；展示备份 ref、旧 SHA 和候选 SHA，取得对改写 fork master 的明确授权。
5. 仅在授权后，用绑定旧完整 SHA 的 force-with-lease 更新 origin/master；禁止无保护 force，禁止推送 upstream。若 lease 失败停止，不覆盖并发提交。
6. 更新本地 master 并验证其与 origin/master 一致、upstream SHA 是其祖先。
7. 回滚保留候选分支和备份；恢复远端同样需要新的人工授权和精确 lease。

## 验证策略与阻塞条件

先在未修改的上游基线上跑构建/测试，区分上游问题与移植回归。已知本机 rustc 1.93.1，此前构建中 Cooke 的 if-let guard 报不支持；不能据此把所有错误归咎于移植，也不能声称已确定最低 Rust 版本。使用经验证的兼容工具链，并记录版本，不批量降级上游语法。

运行库测试、可选 sony-xml 测试、GoPro wire-format/规范化/DOP 回归、DJI 消费契约、Python 和 WASM 的独立 manifest 检查/构建、真实样例与 rimecut-public 集成检查。根目录 cargo test --all 不会自动覆盖所有独立 bin crate。FFmpeg native 依赖工具、缺失平台工具链或真实样例的检查分别记录为未验证，不以绿色根测试替代。

本设计不承诺工具链、消费方和媒体样例尚未取得时即可完成发布。
