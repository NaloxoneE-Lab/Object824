# `o824-mock` npm 包的设计（D 方案：WASI）

状态：**已批准**（2026-11-02） · 2026-11-02 · 关联 §19.5、§18.4、`docs/M5-REPORT.md`

## 1. 要解决的问题

规格 §19.5 承诺："UI 开发者 `npx o824-mock` 就能开发全部界面，**不需要 Windows**、不需要坏掉的 DSH。"

今天只有 `o824 mock`（Windows exe），macOS/Linux 的开发者用不了。这个包就是补上那个承诺。

**不做的事**：不发布到 npm 公共 registry（先走 Release tarball）；不做 TS SDK；不做 `o824` 内核本身的跨平台（内核要访问注册表/junction/FSCTL，那是 Windows 专属，P1 之外）。

## 2. 形状

一份 Rust 实现，两个构建产物：

```
crates/o824-mock/src/bin/o824-mock.rs       新增入口（~80 行，无协议逻辑）
  ├─ cargo build              → o824-mock.exe            宿主，等价于 `o824 mock`
  └─ cargo build --target wasm32-wasip1 --release
                              → o824-mock.wasm  234 KiB
```

npm 包内容：

```
packages/o824-mock/
  package.json           name/version/bin/engines/files
  bin/o824-mock.mjs      ~110 行 launcher（见 §3）
  o824-mock.wasm         ← 构建产物，**不入库**
  transcripts/*.jsonl    3 个内置录制（构建时从 spec/ 复制）
  README.md              给 UI 开发者看的一页
```

**校验和**：构建脚本同时产出 `o824-mock-<ver>.tgz` 与 `SHA256SUMS`，Release 页面两个都挂。

## 3. launcher 的契约（唯一有自由度的地方）

launcher 只做四件事，全部有明确规则：

1. **参数透传**：`--transcript/--delay/--fail-at/--protocol/--help` 原样交给 guest，除 `--transcript` 的值（见 2）。
2. **transcript 定位**：
   - 值以 `.jsonl` 结尾或含 `/`/`\` → 视为主机路径：`resolve()` 成绝对路径，把它**所在目录**预开到 guest 的 `/t`，参数改写成 `/t/<basename>`。
   - 否则视为内置名（`doctor-clean`/`doctor-findings`/`plan-apply`）→ 预开包的 `transcripts/` 目录。名字不认识时 launcher 自己报错并退出 2，**不让 guest 去猜**。
3. **stdio/环境/退出码**：直接继承。guest 的退出码就是 launcher 的退出码（0 正常、2 用法、8 transcript 不可读）。
4. **Node 版本闸门**：`process.versions.node` 主版本 < 20 时，打印一句明确的话并退出 2，不进入 WASI。

**静音范围**：只静音 Node 的 `WASI is an experimental feature` 这一条警告，其他警告与错误照常上屏。

## 4. 为什么不会漂移

- 跑的是 `o824_mock::Transcript::replay`，launcher 里没有任何帧/JSON-RPC/transcript 解析。
- CI 新增 job `mock-package`：
  - 用 `node --test` 跑 `packages/o824-mock/test/`（无第三方依赖）；
  - 断言三件事：**wasm 的帧 == 录制的帧**、**宿主 exe 的帧 == 录制的帧**、**wasm 的帧 == 宿主 exe 的帧**；
  - 覆盖 `--fail-at`、未覆盖方法、坏参数、缺失 transcript 四条错误路径；
  - **Node 20 与 24 双版本矩阵**（WASI 是实验性 API，这是唯一像样的护栏）。

## 5. 构建与产物

`scripts/build-mock-package.ps1 -Version 0.1.2`：

1. `cargo build -p o824-mock --bin o824-mock --target wasm32-wasip1 --release`
2. 复制 wasm 和三个 transcript 进 `packages/o824-mock/`
3. 写入 `package.json` 的 `version`
4. `npm pack` → `dist/o824-mock-0.1.2.tgz`（`files` 白名单里必须有 wasm；缺失时 `prepack` 守卫直接失败，不产出坏包）
5. 生成 `dist/SHA256SUMS`

`.gitignore` 忽略 `packages/o824-mock/o824-mock.wasm` 与 `dist/`——**产物不入库**，理由见 §7。

## 6. 发布

GitHub Release（tag `v0.1.2`）挂两个资产：`o824-mock-0.1.2.tgz`、`SHA256SUMS`。

开发者安装：

```bash
npm i -D https://github.com/NaloxoneE-Lab/Object824/releases/download/v0.1.2/o824-mock-0.1.2.tgz
npx o824-mock --transcript doctor-clean
```

**发布这一步由用户在网页上完成**（2026-11-02 决定）：创建 Release 与上传资产都要 GitHub API 权限，而本机只有一把 deploy key（只能推 git，不能调 API）。所以脚本只负责把资产准备好并打印一份可照做的清单，上传由人来做。

## 7. 关键取舍

**wasm 不入库。** 我们刚吃过一次亏：仓库里的 `o824.exe` 比 Go 源码旧，让 8 个 plan golden 全红了一次。二进制入库就一定会出现"源码改了、二进制没重建"的漂移。所以：产物只存在于 Release 资产与本地 `dist/`，CI 每次都从源码重建并验证。

**代价**：克隆仓库后不能直接 `npm i ./packages/o824-mock`，要先跑一次构建脚本。写进 README。

**不做 `npx o824-mock`（未发布到 registry）**：那需要一个 npm 账号 + token。包的结构不为这件事做任何妥协——以后发布 registry 只是多一条 `npm publish`，包内容不变。

## 8. 要改的文档

- 设计文档 §19.5：把"`npx o824-mock`"改成两条实际可用的安装方式（Release tarball / 未来的 registry），并写明"不需要 Windows"是靠 WASI 实现的。
- README：加一节"用 mock 开发 UI"，含那段 20 行的客户端示例。
- `docs/CHANGELOG`（Rust 侧）：记录本次新增。

## 9. 验收判据（可机器判定）

```
pwsh -File scripts/build-mock-package.ps1 -Version 0.1.2   → 产出 tgz + SHA256SUMS，wasm 在包内
node --test packages/o824-mock/test                        → 全绿（含三方帧比对）
GitHub Actions job mock-package                            → Node 20 与 24 都绿
解包后 npm i -D <tgz> 到一个空目录再跑 npx o824-mock --transcript doctor-clean → 7 帧与录制一致
```

## 10. 未决

- Release 上传由谁做（见 §6）。
- ~~是否把 `o824-mock.exe` 一起挂~~ —— **一起挂**（2026-11-02 决定），构建脚本把它一并放进 `dist/`。
