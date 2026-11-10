# radicom-samples

使用 [radicom](https://github.com/cocosip/radicom) 和
[radicom-codecs](https://github.com/cocosip/radicom-codecs) 的 Rust 示例。
参考 [fo-dicom-samples](https://github.com/fo-dicom/fo-dicom-samples) 的场景，使用 Rust 公共 API 重新实现。
不包含 GUI、图像显示、布局渲染、PNG/JPEG 图片导出或物理打印。

## 环境与构建

Rust **1.89+**。依赖 crates.io 发布的 **0.2 系列** radicom、radicom-network、radicom-codecs，
无需检出两个库，也不需要 C/C++ 编解码库。未启用 `render` 功能。

```powershell
cargo build --locked --examples
cargo test --locked --all-targets
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

每个示例均支持 `cargo run --example <名称> -- --help`；带子命令的示例可以继续查看子命令帮助。
以下命令从本仓库根目录运行，PowerShell、Bash 均可使用 `cargo` 命令。
服务默认仅监听回环地址，按 Ctrl+C 结束。`RUST_LOG=info` 可启用库日志。

## 示例对照

| 示例 | 内容 | fo-dicom 参考场景 |
|---|---|---|
| `create_dicom` | 创建 16×16 合成灰度 DICOM | 基础文件/数据集 API |
| `inspect_dicom` | 输出标签、传输语法和像素元数据 | ConsoleTest |
| `echo_scu` | C-ECHO 连通性检查 | Verification |
| `store_scu` / `store_scp` | 发送和持久化接收 DICOM | C-Store SCU/SCP |
| `qr_scu` / `qr_scp` | Study Root C-FIND、C-GET、C-MOVE | QueryRetrieve SCU/SCP |
| `worklist_scu` / `worklist_scp` | MWL 查询与 MPPS 开始/完成/中止 | Worklist SCU/SCP |
| `dicom_media` | 创建及遍历 DICOMDIR | DICOM Media |
| `dicom_compare` | 递归比较标签、序列及二进制值 | DICOM Compare 的命令行版本 |
| `print_scu` / `print_scp` | 灰度 Print Management 协议及内存作业记录 | Print SCU/SCP 的非渲染部分 |
| `wado` | HTTP WADO-URI 返回 DICOM 文件 | Wado 的 `application/dicom` 分支 |
| `transcode` | 注册 codecs 并转换 DICOM 传输语法 | radicom-codecs 补充示例 |

示例入口在 `examples/`；多个入口和测试实际共享的代码放在 `src/`。
测试用数据由代码合成，不复制参考仓库里的患者图像。

## 1. 文件与合成数据

先创建输入目录：

```powershell
mkdir data
cargo run --example create_dicom -- data/sample.dcm
cargo run --example inspect_dicom -- data/sample.dcm
```

默认 Patient ID 为 `SAMPLE001`，Patient Name 为 `SAMPLE^SYNTHETIC`。
Study / Series / SOP Instance UID 分别为 `2.25.100`、`2.25.101`、`2.25.102`；
Study Date 为 `20260101`。像素是 0 到 255 的梯度，没有真实患者数据。
可用 `--instance-uid 2.25.103` 生成第二个实例。

所有输出都拒绝覆盖已有文件；重复执行时请指定新路径。`create_dicom` 的输出父目录需先存在。
生成文件仅供示例和测试，不用于诊断。

## 2. C-ECHO 与 C-STORE

终端 A：

```powershell
cargo run --example store_scp -- --output received
```

终端 B：

```powershell
cargo run --example echo_scu
cargo run --example store_scu
```

默认地址 `127.0.0.1:11112`，Called AE `SAMPLES_SCP`，Calling AE `SAMPLES_SCU`。
SCP 支持 `--address`、`--ae`；SCU 支持 `--address`、`--calling-ae`、`--called-ae`。
`store_scu` 省略文件参数时发送内置的 16×16 合成实例（与 `create_dicom` 内容相同，SOP Instance UID `2.25.102`），
因此不必先准备输入文件；给出路径时改为发送这些文件，可多个。SCP 默认声明 SC/CT/MR 存储类，可通过重复的 `--sop-class <UID>` 扩充。

接收文件以校验后的 SOP Instance UID 命名，如 `received/2.25.102.dcm`。
只有写入成功才返回成功状态；重复实例、UID 不一致和写入失败均返回失败，不覆盖旧文件。
内置实例的 UID 固定为 `2.25.102`，所以重复发送前需换一个 `--output` 目录（如 `received2`）。
发送端和 QR 示例在启动时注册 codecs，可将压缩输入转换成对端协商的非压缩语法；
本机流程已验证 RLE 输入的 C-STORE 与 C-GET 像素保持一致。

## 3. 查询与检索

终端 A 启动内置的 2 个实例（Study `2.25.100`、Series `2.25.101`、Instance `2.25.102`/`2.25.103`）：

```powershell
cargo run --example qr_scp
```

用 `--input <目录>` 改为服务自己的 `.dcm` 快照。

终端 B 查询并通过 C-GET 接收：

```powershell
cargo run --example qr_scu -- find
cargo run --example qr_scu -- find --patient-name "SAMPLE^*" --study-date 20260101-20261231
cargo run --example qr_scu -- find --level series --study 2.25.100
cargo run --example qr_scu -- find --level image --study 2.25.100 --series 2.25.101
cargo run --example qr_scu -- get
```

默认 QR 地址 `127.0.0.1:11113`、Called AE `QR_SCP`。
连接参数写在子命令之前，如 `qr_scu --address 127.0.0.1:11113 find`。
GET/MOVE 省略 UID 时使用内置示例的 Study/Series/Instance UID，不必先复制 FIND 的结果。

C-MOVE 会另建关联向目标存储 SCP 发送实例。目标必须在 `qr_scp` 的 `--destination AE=host:port` 中配置，
默认 `SAMPLES_SCP=127.0.0.1:11112`，传入自己的映射会替换该默认值；未知 AE 返回失败，不直接连接请求中的任意地址。
先在另一个终端启动新的目标目录，避免上一步 STORE 的重复实例：

```powershell
cargo run --example store_scp -- --output moved
```

停止之前占用 11112 的 `store_scp` 后，再运行：

```powershell
cargo run --example qr_scu -- move
```

`--level study|series|image` 选择层级。Series 查询要求 `--study`，Image 查询要求 `--study` 和 `--series`；
GET/MOVE 还要求当前层级的唯一 UID：Study 要 `--study`，Series 再要 `--series`，Image 再要 `--instance`。
GET 默认协商 SC/CT/MR 的反向 C-STORE，可用重复的 `--sop-class` 指定实际存储类。

查询示例的匹配子集：Patient Name/ID、Accession Number、Modality 支持区分大小写的 `*`、`?`；
UID 精确匹配（FIND 支持反斜杠分隔的 UID 列表）；Study Date 支持 `YYYYMMDD`、闭区间和单侧开区间。
其他已声明的键仅用于返回；未支持的键会明确失败。响应中的成功、Pending、失败及子操作计数都可在终端查看。
文本/日期过滤只适用于 FIND；GET/MOVE 只接受唯一 UID 标识，不接受人口学条件。
0.2 的 provider 请求未暴露发起方 AE，示例的 MOVE 子操作不会编造 Move Originator AE；
需要严格依赖该字段进行关联的第三方系统仍需单独核对。

## 4. Worklist 与 MPPS

终端 A：

```powershell
cargo run --example worklist_scp
```

终端 B：

```powershell
cargo run --example worklist_scu -- find --patient "SAMPLE*" --modality CT --date 20261101-20261130
cargo run --example worklist_scu -- mpps
cargo run --example worklist_scu -- mpps --final-status discontinued
```

默认地址 `127.0.0.1:11115`、Called AE `WORKLIST`。
演示工作项为 `SAMPLE001`、SPS ID `SAMPLE_SPS_1`，计划日期 `20261109`，Modality `CT`。
MWL 查询包含 Scheduled Procedure Step Sequence，并只返回请求的字段。
示例查询按 Patient ID、Modality、计划日期匹配；provider 同时演示其他声明字段的匹配及日期范围。
MPPS 通过 N-CREATE 建立 `IN PROGRESS`，再用 N-SET 进入 `COMPLETED` 或 `DISCONTINUED`。
可用 `--instance-uid` 指定 MPPS UID；不存在对象、重复创建、非法状态或无效 SPS 引用均拒绝。

## 5. DICOMDIR 与比较

```powershell
cargo run --example dicom_media -- write data media
cargo run --example dicom_media -- read media
cargo run --example dicom_compare -- data/sample.dcm data/sample.dcm
```

`dicom_media write` 需要全新的目标目录，复制实例到 `IMAGES/00000001` 等有效 File ID 后写入 DICOMDIR。
`read` 遍历 Patient → Study → Series → Instance，并检查引用文件是否存在。
失败的创建操作可能留下部分文件集，请检查后换用新的目标目录。

比较包含嵌套序列、VR、缺失标签和二进制内容；输出差异的标签路径。
退出码：相同为 0、有差异为 1、读取错误为 2。
不比较文件元信息/传输语法标识，不解压像素进行语义比较：不同压缩表示的 Pixel Data 可以被报告为不同。
网络协商转换也可能把 Pixel Data 的 VR 从 OB 改为 OW；即使像素相同，包含 VR 的比较仍会报告差异。

## 6. 打印协议（不渲染）

终端 A：

```powershell
cargo run --example print_scp
```

终端 B：

```powershell
cargo run --example print_scu
```

默认地址 `127.0.0.1:11114`、Called AE `PRINT`。
SCU 使用合成 8 位单色像素，演示 Film Session / Film Box N-CREATE，
读取服务器返回的 Image Box 引用，N-SET 图像，N-ACTION 提交 Film Box，最后 N-DELETE 清理对象。
SCP 检查引用、像素形状和生命周期，仅支持一份、A4、`STANDARD\1,1` 的最小灰度流程。
作业记录成功不表示物理打印成功；不生成布局或调用操作系统打印机，也不宣称覆盖打印机全部协商方式。

## 7. WADO-URI（只返回 DICOM）

```powershell
cargo run --example wado
```

默认服务内置的 2 个实例；用 `--directory <目录>` 改为服务自己的 `.dcm` 快照。

在另一终端下载：

```powershell
curl.exe --fail -H "Accept: application/dicom" "http://127.0.0.1:8080/wado?requestType=WADO&studyUID=2.25.100&seriesUID=2.25.101&objectUID=2.25.102&contentType=application%2Fdicom" --output downloaded.dcm
```

Bash 使用 `curl`。服务通过 `--bind` 指定地址，默认 `127.0.0.1:8080`。
必须显式指定 `contentType=application/dicom`；检查 Accept 与 contentType 是否相容。
三个 UID 必须同时匹配。缺省 transferSyntax 返回 Explicit VR Little Endian；
`transferSyntax=*` 保留源文件原始字节，指定 transfer-syntax UID 可调用已注册 codecs 转码。
不支持的语法或媒体类型返回 HTTP 错误，不静默回退；非法、重复或不支持的查询参数也会拒绝。
不支持渲染、窗口、缩放、帧选取或匿名化控制。

## 8. radicom-codecs 转码

```powershell
cargo run --example transcode -- data/sample.dcm rle.dcm --syntax 1.2.840.10008.1.2.5
cargo run --example transcode -- rle.dcm restored.dcm --syntax 1.2.840.10008.1.2.1
cargo run --example dicom_compare -- data/sample.dcm restored.dcm
```

启动时调用 `radicom_codecs::install()` 一次，将十二种压缩语法注册到 radicom。
目标由 `--syntax <UID>` 明确选择；源文件保留，目标已存在则失败。
有损目标必须另外传 `--allow-lossy`，像素值会发生变化。

| 语法 | UID |
|---|---|
| RLE Lossless | `1.2.840.10008.1.2.5` |
| JPEG Process 1 / Process 2&4 | `1.2.840.10008.1.2.4.50` / `.51` |
| JPEG Lossless Process 14 / SV1 | `1.2.840.10008.1.2.4.57` / `.70` |
| JPEG-LS Lossless / Near-Lossless | `1.2.840.10008.1.2.4.80` / `.81` |
| JPEG 2000 Lossless / Lossy | `1.2.840.10008.1.2.4.90` / `.91` |
| HTJ2K Lossless / Lossless RPCL / Lossy | `1.2.840.10008.1.2.4.201` / `.202` / `.203` |

后缀 `.51` 等表示同一行首个 UID 的完整共同前缀；传给命令时使用完整 UID。
实际支持的位深、颜色和帧结构由 codec 决定，失败会原样报告；不会更改压缩算法来掩盖错误。

## 验证边界与示例限制

功能测试使用临时目录及真实本机 TCP/HTTP，覆盖文件写入拒绝覆盖、DICOMDIR 实际引用、
嵌套/二进制比较、五个无损 codec 家族的文件持久化往返、C 服务闭环、
MWL/MPPS、打印对象流程及 HTTP 内容协商/错误请求。
CI 配置包含 Linux/Windows stable 及 Rust 1.89 编译检查；本地结果不能替代远程 CI。

这些是受信本地环境的学习示例，未配置 TLS、身份认证、数据库或生产服务管理。
QR/WADO 不给 `--input`/`--directory` 时使用内置的 2 个合成实例，给了目录则在启动时读取快照、新增文件需重启；
目录扫描仅识别普通 `.dcm` 文件，不跟随符号链接。
QR 的实例数上限为 65,535。Worklist/打印对象保存在内存且由关联共享，进程重启后丢失；
provider 回调没有关联身份，示例不提供多租户对象隔离。
真实 PACS、真实打印机、第三方互操作，以及全部 codec/位深/颜色组合需要单独验证。

## 参考与归属

感谢 fo-dicom / fo-dicom-samples 的示例设计。参考项目采用 MS-PL；本仓库未直接复制其 C# 实现或患者图像。
radicom 及 radicom-codecs 的实现和第三方数据许可见各自仓库。
