# DOH Proxy

基于 Rust 实现的 DNS over HTTPS (DOH) 代理，内部支持 ECH (Encrypted Client Hello)。

## 功能

- **DOH DNS 解析**：通过 HTTPS 加密 DNS 查询，防止 DNS 污染
- **ECH 支持**：加密 TLS 握手中的 SNI 字段（需要服务器支持）
- **多 DOH 服务器**：支持 Cloudflare、Canadian Shield、Google、Quad9、DNSPod、腾讯 DNS、阿里 DNS
- **跨平台**：支持 Windows、macOS、Linux、Android、iOS
- **公共 ECH 入口策略**：默认将 `linux.do`、`ldstatic.com`、`idcflare.com` 及子域名的 DNS 查询替换为 `cloudflare-ech.com`，支持关闭及自定义域名；真实 Host、TLS SNI、证书校验和缓存键不变。关闭时也不使用公共入口兜底，仍查询原域名的 HTTPS/ECH 记录；主动公共查询失败不会重复请求同一入口。
- **并发与连接复用**：DNS 查询单飞、握手竞速和预连；网关共享 h1/h2 连接池，换网、优选代数变化和停止时失效旧连接，仅对允许安全重放的请求重试。
- **额外覆盖域名**：`extra_covered_hosts` 中的完整主机名及其子域和 `linux.do` 共用 Selector / EdgePin 赢家，仅按域名标签边界匹配。应用登记 `idcflare.com`、`ldstatic.com` 后，两论坛与静态资源统一走赢家；DNS / ECH 覆盖不改变真实 Host、证书校验或应用的 Cookie 站点隔离。
- **启动与诊断**：启动成功前同步绑定端口，网关拒绝回环 Host，FFI 可读取建连、握手、连接复用与解析等待统计。

## 项目结构

```
rust/
├── Cargo.toml          # 项目配置
├── src/
│   ├── lib.rs          # 库入口
│   ├── main.rs         # 可执行文件入口（桌面平台）
│   ├── ffi.rs          # FFI 接口（移动平台）
│   ├── proxy.rs        # HTTP CONNECT 代理服务器
│   ├── dns.rs          # DOH DNS 解析器
│   ├── ech.rs          # TLS 连接器（ECH 支持）
│   └── error.rs        # 错误类型
└── README.md
```

## 编译

### 前置要求

- Rust 1.70+
- 对于 Android：cargo-ndk、Android NDK

### 桌面平台（Windows/macOS/Linux）

推荐使用一键脚本：

```bash
# Windows PowerShell
.\scripts\build_desktop.ps1

# macOS/Linux
./scripts/build_desktop.sh
```

或手动编译：

```bash
cd rust

# Debug 编译
cargo build

# Release 编译
cargo build --release

# 可执行文件位于
# Windows: target/release/doh_proxy_bin.exe
# macOS/Linux: target/release/doh_proxy_bin
```

### Android

推荐使用一键脚本（自动编译 + 复制到 jniLibs）：

```bash
# Windows PowerShell
.\scripts\build_android.ps1

# macOS/Linux/Git Bash
./scripts/build_android.sh
```

或手动编译：

```bash
# 安装 Android 目标
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android

# 安装 cargo-ndk
cargo install cargo-ndk

# 编译所有架构
cd rust
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 build --release

# 库文件位于
# target/aarch64-linux-android/release/libdoh_proxy.so
# target/armv7-linux-androideabi/release/libdoh_proxy.so
# target/x86_64-linux-android/release/libdoh_proxy.so
# target/i686-linux-android/release/libdoh_proxy.so
```

### iOS

```bash
# 安装 iOS 目标
rustup target add aarch64-apple-ios

cd rust
cargo build --release --target aarch64-apple-ios

# 静态库位于 target/aarch64-apple-ios/release/libdoh_proxy.a
```

## 运行（桌面平台）

```bash
# 默认配置（Cloudflare DOH，自动分配端口）
./target/release/doh_proxy_bin

# 指定端口
./target/release/doh_proxy_bin 8080

# 指定 DOH 服务器
./target/release/doh_proxy_bin 8080 --doh https://dns.alidns.com/dns-query

# 优先使用 IPv6
./target/release/doh_proxy_bin 8080 --ipv6
```

### 命令行参数

| 参数 | 说明 |
|------|------|
| `<port>` | 监听端口（0 = 自动分配） |
| `--doh <url>` | DOH 服务器 URL |
| `--ipv6` | 优先使用 IPv6 地址 |
| `--ech-dns-policy <json>` | 公共入口策略，如 `{"enabled":true,"custom_domains":["example.com"]}` |

### 支持的 DOH 服务器

| 名称 | URL |
|------|-----|
| Cloudflare | `https://cloudflare-dns.com/dns-query` |
| Canadian Shield | `https://private.canadianshield.cira.ca/dns-query` |
| Google | `https://dns.google/dns-query` |
| Quad9 | `https://dns.quad9.net/dns-query` |
| DNSPod | `https://doh.pub/dns-query` |
| 腾讯 DNS | `https://dns.pub/dns-query` |
| 阿里 DNS | `https://dns.alidns.com/dns-query` |

### 公共 ECH 入口配置

应用在 ECH 设置中管理此策略，关闭公共入口不等于关闭站点自身 ECH。自定义项仅接受完整域名，不接受 URL、端口、IP 或通配符，每项同时匹配其子域名。

Android/iOS 与桌面 FFI 的启动 JSON 使用 `ech_dns_policy` 字段；桌面独立进程使用 `--ech-dns-policy` 传递相同 JSON。独立 FFI 查询通过 `doh_proxy_set_ech_dns_policy_json` 更新策略，返回 `0` 表示成功，`-1` 表示参数错误。策略变更隔离解析器及缓存，并由应用重启网关，已有连接保留启动时策略快照。

本地 Selector 优选、加速路由表和节点列表仍按既有优先级工作；公共入口替换只发生在实际 DNS 查询层，不覆盖这些本地网络扩展。

MITM 范围由应用的 `mitm_hosts` 独立限定；Linux WebView 为 `linux.do`、`idcflare.com`、`ldstatic.com` 及其子域使用代理签发的叶子证书，已知静态资源主机在代理启动后预放行。范围外仍走端到端 TLS 隧道，不因登记优选覆盖域名而放宽证书信任。

修改该策略接口后需重新构建目标平台原生库和桌面可执行文件，不能混用旧二进制。公网 ECH 冒烟测试位于 `tests/cloudflare_ech.rs`，默认忽略，需要显式运行。

## 测试

使用 curl 测试代理：

```bash
# 启动代理
./target/release/doh_proxy_bin 8080

# 使用代理访问网站
curl -x http://127.0.0.1:8080 https://example.com
```

## Flutter 集成

### Android

使用一键脚本编译并部署：

```bash
# Windows PowerShell
.\scripts\build_android.ps1

# macOS/Linux/Git Bash
./scripts/build_android.sh
```

脚本会自动编译所有架构并复制到 `android/app/src/main/jniLibs/`。

Flutter 中使用：

```dart
import 'package:fluxdo/services/network/doh_proxy/doh_proxy.dart';

// 启动代理
final service = DohProxyService.instance;
final success = await service.start(
  preferredPort: 0,  // 自动分配
  preferIPv6: false,
  dohServer: 'https://dns.alidns.com/dns-query',
);

if (success) {
  print('代理已启动，端口: ${service.port}');
}

// 停止代理
await service.stop();
```

### 桌面平台

Windows 使用进程内 FFI，消费 `windows/runner/native/doh_proxy.dll`，通过主仓 `dart run tool/project_tasks.dart native:prepare windows --release` 构建和暂存，不再依赖代理可执行文件。其他桌面后端可使用进程 fallback；配置和预连主机参数与 FFI 保持一致。

## FFI 接口

移动平台及 Windows 通过 FFI 调用 Rust 库：

```c
// 启动代理，返回端口号（-1 表示失败）
int doh_proxy_start(int port, int prefer_ipv6);

// 启动代理（指定 DOH 服务器）
int doh_proxy_start_with_server(int port, int prefer_ipv6, const char* doh_server);

// 按 JSON 配置启动；返回时已绑定端口，端口冲突立即返回 -1
int doh_proxy_start_with_config_json(const char* config_json);

// 读取累计网络统计 JSON；结果使用 doh_proxy_free_string 释放
char* doh_proxy_net_stats();
void doh_proxy_free_string(char* value);

// 停止代理
void doh_proxy_stop();

// 检查是否运行中（1 = 是，0 = 否）
int doh_proxy_is_running();

// 获取代理端口（0 = 未运行）
int doh_proxy_get_port();

// 初始化日志
void doh_proxy_init_logging();
```

## 架构说明

```
┌─────────────────────────────────────────────────────────────┐
│                      Flutter App                             │
├─────────────────────────────────────────────────────────────┤
│  DohProxyService (lib/services/network/doh_proxy/)          │
│    ├── 桌面平台：启动进程                                    │
│    └── 移动平台：FFI 调用                                    │
├─────────────────────────────────────────────────────────────┤
│                    Rust DOH Proxy                            │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐          │
│  │ DohProxy    │  │ DnsResolver │  │ DohTls      │          │
│  │ Server      │──│ (DOH)       │──│ Connector   │          │
│  │ (CONNECT)   │  │             │  │ (ECH)       │          │
│  └─────────────┘  └─────────────┘  └─────────────┘          │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
              ┌─────────────────────────┐
              │  DOH Server             │
              │  (Cloudflare/阿里/...)  │
              └─────────────────────────┘
```

## 工作原理

1. **HTTP CONNECT 代理**：客户端发送 `CONNECT host:port` 请求
2. **DOH DNS 解析**：通过加密 HTTPS 查询 DNS，获取目标 IP
3. **TCP 隧道**：建立到目标服务器的 TCP 连接
4. **双向转发**：在客户端和服务器之间转发数据
5. **ECH（可选）**：如果服务器支持，加密 TLS 握手中的 SNI

## 依赖

- rustls 0.23+ (TLS + ECH 支持)
- hickory-resolver (DOH + HTTPS 记录)
- tokio (异步运行时)

## License

MIT
