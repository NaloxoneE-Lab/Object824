# tui 形态的判据（2026-11-03 已定并落地）

> 用户已按建议拍板：F23 只查 PATH 上的 node；F25 只看有没有 `dispose` 行；F27 暂不做；
> `dst doctor` 的 ✗ 只取环境相关行。下面是判据本身与落地位置。
> 依据都是 2026-11-03 在本机的实测（`@deepseek-harness-tui/dsh-tui` 0.18.0，Node v24.8.0）。

## 零、三个前提（F03/F16 的教训）

1. **只报能证明的**。判据必须是机器状态的直接读数；证明不了就**沉默**，不要猜。F03 前两版都是"看起来合理但没验证"，一上真机就误报。
2. **委派优先**。dsh-TUI 自带 `doctor` / `safe` / `safe --rescue` / `update`，它比我们更懂自己。我们先调、解析、只补它不说的。
3. **不自动修**。所有 tui 判据都是"告诉人"，没有一条会让内核去改第三方应用的文件。

## 一、必须先知道的两个真机事实

**① `dst doctor` 会失败却 exit 0。**

```
✗ DEEPSEEK_API_KEY: 未设置——环境变量与 DSH 凭据库中都没有 DEEPSEEK_API_KEY
exit=0
```

**绝对不能拿退出码当判据**——这是委派最容易踩的坑。只能解析行的首个字形（`✓`/`✗`）。它也没有 `--json`（`dst doctor --json` 被忽略）。

**② 文档对 tui profile 的假设是错的。** 真实布局：

```
~/.dsh/profiles/dsh-tui/
├── package.json            # 只有 bundles + 一个依赖
├── cordis.yml / cordis.patch.yml
├── pnpm-workspace.yaml     # nodeLinker: hoisted, autoInstallPeers: false
└── node_modules/@deepseek-harness-tui/dsh-tui/   # 是**副本**，不是链接
```

整个 `node_modules` 里**没有任何 `.node`、没有 `binding.gyp`、没有 `.modules.yaml`**。也就是说：npm 安装形态下，tui profile 里**没有依赖树**——peer 依赖来自别处。文档里 F24 的判据（"pnpm 拦了构建脚本 → 原生模块缺失"）**在这个布局下无处可查**。

（顺带修正我上一轮的说法：`~/.dsh-tui/restart.log` **是存在的**。我早上查看时那个目录刚建、应用还没跑过，所以没有这个文件。文档这条是对的，我的推断是错的。）

## 二、F23：Node 版本门槛

| 项 | 内容 |
|---|---|
| **来源** | tui 包的 `package.json` → `engines.node`（实测值：`^22.19 \|\| >=24`） |
| **对照物** | 实际会运行它的那个 Node——即 PATH 上的 `node --version`（`dst` 本质是 JS，由 PATH 的 node 执行） |
| **判据** | 版本不在范围内 → finding。params：`{range, node}` |
| **沉默条件** | 包没声明 `engines.node` → 什么都不报（没声明就没有可违反的） |
| **可修性** | 无自动修。换 Node 是用户的事；内核绝不代装 Node |
| **交叉验证** | 若 `dst doctor` 的 `node:` 行是 `✗`，采用它的说法并标注来源是"tui 自己" |

**要你确认**：只查 PATH 上的 node，还是要查"每个可能的 node"（例如用户用了 nvm、`DST_NODE` 环境变量）？我建议**只查 PATH**，并在文案里写明"按 PATH 上的 node 判断"。

## 三、F24：原生模块缺失——**建议这一条先不做**

| 方案 | 可行性 |
|---|---|
| A. 按文档：查 profile 里的构建产物 | **不可行**。实测 profile 里没有依赖树，没有 `.node`、没有 `binding.gyp` |
| B. 委派：只看 `dst doctor` 的 `pnpm:` 行 | 可行但很薄——它只说"pnpm 21.0 存在/不存在"，不检查构建脚本 |
| C. 自己算：扫全局 DSH 包里"有 `install`/`postinstall` 但没产物"的包 | 可行，但那是**DSH 的**问题，属于已有 F17 的范围，不该挂在 tui 名下 |

**我的建议**：F24 **暂时不实现**，在文档里注明"npm 安装形态下无依赖树可查；等真的碰到一次原生模块故障，拿到证据再定判据"。这比编一条查不出东西的检查诚实。

## 四、F25：上次退出/更新没走完

| 项 | 内容 |
|---|---|
| **来源** | `~/.dsh-tui/restart.log`（实测 244 字节） |
| **实测格式** | `<ISO 时间> pid=<n> <事件> <json>`，一次退出是一组三行：`funnel: process exit requested` → `signal: received` → `dispose: root disposed` |
| **判据** | **最后一组**只有 `process exit requested`、没有 `dispose: root disposed` → 那次退出没走完（被杀 / 更新中断 / 断电） |
| **params** | `{lastEvent, at}` |
| **沉默条件** | 文件不存在或为空 → 不报（没有记录不是故障） |
| **可修性** | B（委派）：`dsh-tui update` 或 `safe --rescue`。**不自动跑**，只在输出里给出命令 |

**要你确认**：`dispose` 超时算不算失败？实测那组 `dispose: root disposed {"ms":1273}` 用了 1.27 秒。我建议**只看"有没有这一行"**，不看耗时——耗时阈值需要基线，我没有。

## 五、新判据（暂定 F27）：tui 与 DSH 的 peer 兼容

这一条是**文档里没有、但真机数据里明摆着的**：

tui 的 `peerDependencies` 把每个 `@deepseek-ai/dsh-*` 都钉在一串**显式允许的版本**上，末端是 `0.2.1-rc.5`：

```
"@deepseek-ai/dsh-agent": "^0.1.0-rc.6 || ... || 0.2.1-rc.2 || 0.2.1-rc.3 || 0.2.1-rc.4 || 0.2.1-rc.5"
```

而本机 desktop 是 `0.2.1-rc.3`、web 是 `0.2.1-rc.5`——**都在名单里**，所以现在没问题。但 DSH 一旦升到 `rc.6`，tui 就会对着一份它没声明支持的 DSH 运行。

| 项 | 内容 |
|---|---|
| **来源** | tui 的 `peerDependencies` × 已发现的 DSH 形态版本 |
| **判据** | 某个 `@deepseek-ai/dsh-*` 的 peer 范围**不接受**任一在跑的 DSH 版本 → finding |
| **和 F11 的区别** | F11 说"两个形态版本不同"（可能完全无害）；这条说"tui 跑在这份 DSH 上是不被支持的"（是真的会坏） |
| **params** | `{peerPackage, range, installed}` |
| **代价** | 要实现带预发布号的 semver 范围判断。`^0.1.0-rc.6` 对 `0.2.1-rc.3` 是**不满足**的（major 0 的 caret 只到 0.1.x），所以必须按 OR 列表逐项判断，不能简化 |

**要你确认**：这条值不值得现在做？它需要一个正确的 semver 实现（含预发布比较规则），是本草案里最贵的一项。如果只做 F23 + F25，tui 的体检就已经覆盖"起不来"的两个主因。

## 六、明确不做

- **不启停 tui**（§21.5 已定：`E_UNSUPPORTED`，TUI 要在真实终端里跑）
- **不代管插件**（§21.6：只读展示）
- **不自动修任何一条**（包括 F25 的 `dsh-tui update`——那是第三方自己的写入）
- **不把 `dst doctor` 的 ✗ 全部当故障**：它会把"没配 API key"也标 ✗，那是配置需求，不是安装故障。

**要你确认**：`dst doctor` 报的 ✗ 行怎么呈现？三个选项：
1. 全部转成 finding（诚实，但每次体检都会因为没配 key 而报警）
2. 只取与环境/安装相关的几行（node / pnpm / profile / launcher 对齐），跳过凭据类
3. 完全不转，只在 `doctor --verbose` 里附上原文

我倾向 **2**：把"起不来"的成因报出来，把"还没配 key"留给应用自己说。

## 七、怎么验证（不碰真实安装）

| 判据 | 验证方式 |
|---|---|
| F23 | 临时 package.json 写一个 `engines.node`，用真的 Node 版本去比；再造一个必定不满足的范围 |
| F25 | 临时 `restart.log`：一组完整的（应沉默）、一组缺 `dispose` 的（应报警）、空的（应沉默） |
| F27 | 用真机的 peer 表 + 伪造成 `0.2.1-rc.6` 的 DSH 版本，断言报警 |
| 委派 | 用一个假的 `dst` 脚本输出固定的 ✓/✗ 文本，断言我们解析出的 finding 与退出码无关 |

所有用例都在临时目录里跑，**不会执行真实的 `dsh-tui update`**。


## 八、落地（2026-11-03）

| 判据 | 实现 | 测试 |
|---|---|---|
| F23 | `crates/o824-verify/src/tui.rs` → `node_range`（读 `engines.node`，比 PATH 上的 node） | 范围内/外、未声明 `engines`、预发布版 |
| F25 | 同文件 → `interrupted_restart`（看 `~/.dsh-tui/restart.log` 最后一组有没有 `dispose` 行） | 完整一组沉默、缺 `dispose` 报警、无日志沉默 |
| F28（委派） | 同文件 → `delegated`（跑 `dsh-tui doctor`，只取 node/pnpm/profile/launcher 的 ✗ 行） | 四个环境键被取、凭据键被跳过 |
| F24 | **不实现**，理由见第三节 | — |
| F27（peer 兼容） | **暂不做**，等需要更严的共存检查时再上 | — |

另外修了一个只有真机会暴露的 bug：`--root` 把 kind 写死成 `desktop`，于是 `--root <tui 包>` 会跳过所有 tui 检查、报"体检通过"。现在 `--root` 从树本身判断形状，判断不了才回落到 desktop（实验室那些坏树仍然按 desktop 诊断）。

真机验证：真实 tui（`i2`）静默（Node 24.8.0 在范围内、`restart.log` 最后一组完整、唯一的 ✗ 是 API key 被过滤）；合成树（`engines: ">=99"` + 缺 `dispose`）两条都报出来。
