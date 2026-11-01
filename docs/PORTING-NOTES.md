# 移植必读：这个内核里"用调试换来的"那些事

写给任何准备用另一种语言重写 O824 内核的人。

代码可以照着重写，**这些结论不能**——它们每一条都是在一台真实机器上撞出来、再用真机复验过的。重写时如果漏掉其中一条，你会重新撞一遍，而用户的 `node_modules` 会被当成本钱。

格式：**结论 → 怎么发现的 → 重写时要注意什么**。

---

## 1. F03（重复 loader entry）的判据不成立

- **结论**：DSH 0.2.1-rc.3 里，`cordis.patch.yml` 的 `id` 指的是**合成后的 loader entry id**（`timer`、`dsh-ops/shell` 这类），不是 bundle 包名。指向不存在的 entry 只打 `patch: entry "X" not found` 警告并继续，**进程照常启动**。用 `insert` 造出真正重复的 entry id，`--dump-config` 合成阶段不报错，真启动（web profile，服务正常监听）也不失败。
- **怎么发现的**：复制真机 profile 成 `probe-desktop`，用宿主 CLI `dsh --profile probe-desktop --dump-config` 跑；再在 web 模板 profile 上真启动。`app.asar` 里搜不到 `duplicate loader entry id` 这个字符串。
- **重写时**：不要"顺手实现"一个看起来合理的重复检查。目前它只是 `severity: warning` 的提示，**不驱动任何自动修改**。判据要重定，重定之前别给用户一个"能修"的假象。

## 2. pnpm 11 的构建白名单（F17）

- **结论**（四条，缺一不可）：
  1. `allowBuilds` 必须是 **map**：`allowBuilds: {"pkg": true}`。写成数组是硬报错 `The allowBuilds field should be an object, but got array`。
  2. `onlyBuiltDependencies` / `neverBuiltDependencies` 在 v11 已移除，写了也不生效。
  3. **git / tarball 依赖用裸包名永远批不下来**，必须写仓库形式：`name@git+https://host/org/repo.git`（不带 `#ref`）或 `name@<url>#<commit>`。
  4. 键里的 URL 要按 pnpm 的规范化拼写：路径含空格时 pnpm 打印的是 `%20` 编码，写裸空格**白名单不生效**。
- **怎么发现的**：本地 `git+file://` 仓库 + 带 `postinstall` 的包，逐条试出来的（真 pnpm 11.28.5）。
- **重写时**：`approvalKeys` 的等价实现必须做 URL 规范化（Go 用 `net/url`）。另外 `pnpm rebuild` / `rebuild --pending` **不会**重跑被拦下的构建，只有 `install` 会。

## 3. 安装动作必须"副本 + 交换"，不能就地

- **结论**：`pnpm install` 会重写整棵 `node_modules`，而快照按设计不含它。所以修复流程是：预检空间（2× node_modules + 256 MiB）→ 建 `<profile>.o824-stage` → 在副本里 install → 校验 → **两次改名交换**（旧的先 `.o824-old`，成功后删，失败改回来）。
- **怎么发现的**：先在副本上量了一次完整安装（35 秒、436 MB、顶层条目与原始一致），再把它做成流程。
- **重写时**：交换必须是"旧树改名 → 新树改名 → 删旧树"，任何一步失败都要能回到起点。不要用"先删后拷"。

## 4. pnpm 的三个运行期陷阱

| 现象 | 根因 | 对策 |
|---|---|---|
| `ERR_PNPM_NO_LOCKFILE` | 新 profile 没有 `pnpm-lock.yaml`，而我们固定加 `--frozen-lockfile` | 只在副本里真有 lockfile 时才冻结 |
| `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY` | pnpm 清空 `node_modules` 前要 TTY 确认（跨卷 store 时会触发） | 固定加 `--config.confirmModulesPurge=false` |
| 用户只看到 `exited with code 1` | **pnpm 把报错写在 stdout**，我们只读 stderr | stderr 为空时回退到 stdout 尾部 |

- **怎么发现的**：真终端跑一遍失败，逐条复现。
- **重写时**：这三条都是"子进程 + 无终端"环境下的行为，任何语言都会撞。

## 5. Windows 上的几个具体事实

- **junction 的 substitute name 由 Windows 决定拼写**：次级卷上会变成 `\??\Global\D:\...`，但它**照样能解析**。判断"链接是否可用"要靠"能不能穿透读到文件"，不要比字符串。
- **`os.Lstat` 不可靠地报告 junction**：判 reparse point 要用 `GetFileAttributes` + `FILE_ATTRIBUTE_REPARSE_POINT`。
- **`cmd /c mklink` 不是安全选项**：Go 不会给无空格的参数加引号，路径里的 `&`/`%` 会被 cmd 重新解析。用 `FSCTL_SET_REPARSE_POINT` 原生创建。
- **UTF-8 BOM**：PowerShell 5.1 的 `Set-Content -Encoding UTF8` 默认写 BOM；Node/pnpm 容忍，但严格的 JSON 解析器会报错。读 profile 文件时先剥 BOM，否则会把好文件报成 F16。
- **`filepath.Abs` 对绝对路径不做 CWD 解析**，但相对路径会——链接目标回滚时如果用相对路径，要按链接所在目录解析，不是进程 CWD。

## 6. 发现与身份

- **版本号在 asar 里，不在 `version` 文件里**：安装根的 `version` 是 Electron 的版本（如 44.7.0），宿主版本要从 `resources\app.asar` 里的 `package.json` 读（0.2.1-rc.3）。
- **profile 在 DSH home，不在安装目录**：`profiles/<name>` 位于 `%USERPROFILE%\.dsh`，和 `Programs\DeepSeek Harness` 是两棵树。`--root` 给的是安装根，`--home` 给的是 profile 家。
- **去重靠 realpath**（`EvalSymlinks` 成功就用它，失败退回 `Clean`，再小写），不是 `Clean` 就够——符号链接和 8.3 短名会让同一棵树出现两次。
- **`--root` 也要有 id**：内核的 plan 拒绝没有 id 的安装，所以按 root 选择时必须推导一个稳定的 `local-<hash>`。

## 7. 协议层的两条约定

- **`op.result` 在 `op.state(done)` 之前**（与 §19.1 的示例相反）。理由：照抄示例的客户端会在 `done` 时停止读取，永远拿不到结果。`o824.job` 提供幂等补取。
- **`seq` 在响应里是顶层字段，在通知里放 `params` 内**——这是 §19.1 的写法，两个实现都要一致。

## 8. 快照范围（§16.1）为什么是那几个文件

`package.json`、`cordis.patch.yml`、`cordis.yml`、`pnpm-lock.yaml`、`pnpm-workspace.yaml`、安装根的 `version`；**不含** `node_modules`。

- `pnpm-workspace.yaml` 是**后来加的**：因为 `allow-builds` 修复会改写它，不纳入快照就等于"修了但回滚不了"。
- 它在 **profile 目录下**（`profiles/<name>/pnpm-workspace.yaml`），不是 home 根——早期版本读错了位置，导致"修了但 pnpm 根本不看"。

## 9. 验收怎么算过

三种机器判据，任何一种语言的重写都必须同时满足：

1. `spec/golden/*.json`：八棵 fixture 树上的 `doctor --json` 与 `plan --json` 输出，规范化后逐字节相等。
2. `spec/transcripts/*.jsonl`：mock 回放的帧序列逐帧相等（`id`/`seq` 按规则重写）。
3. 实验室脚本 `D:\dsh work\broken-dsh-lab\`：真机配置副本上的七种故障，`doctor → repair → doctor` 的退出码与文件效果一致。

第 1、2 条在仓库里；第 3 条需要一台 Windows 机器和一份真实的 DSH 安装。
