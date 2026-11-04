# P4 第一步：发布清单与 ed25519 签名

> 设计依据：§13「不购买代码签名证书」——防篡改靠**自管 ed25519 签名的发布清单 + 逐文件 sha256**。

## 为什么不是"给 exe 签名"

没有代码签名证书，Windows 不会认这个二进制的发布者。能补上的不是"谁编译的"，而是
**"你下载到的这些文件，是不是发布者签名的那一批"**：清单列出每个文件的 sha256，清单本身用
ed25519 签名，公钥单独公布。下载者核两件事——清单是不是那个公钥签的，文件是不是清单里的。

## 三个命令

```bash
o824 manifest-keygen --out key.hex          # 私钥写文件，公钥打出来（贴到发布页）
o824 manifest --root dist --out dist/manifest.json --version 0.2.0 --key-file key.hex
o824 verify-manifest --manifest dist/manifest.json --root dist --public-key <hex>
```

退出码：`0` 通过；`2` 参数或清单格式错；`5` 清单与文件不符（改了/缺了/多了）。

## 实测（2026-11-04，真二进制）

```
manifest: 1 files, signed, written to …/manifest.json
signature: ok (a6f26c0009ce303c)
files: ok (1 checked)                       ← 退出码 0

# 往 exe 尾部追加一个字节后
changed:    o824.exe
files: 1 checked, 1 changed, 0 missing, 0 unexpected   ← 退出码 5
```

清单长这样（节选）：

```json
{
  "format": "o824-manifest/v1",
  "version": "0.1.0-dev",
  "builtAt": "2026-11-04T11:28:06.3347868Z",
  "files": [{ "path": "o824.exe", "sha256": "b271…", "size": 2850816 }],
  "signature": {
    "algorithm": "ed25519",
    "keyId": "a6f26c0009ce303c",
    "publicKey": "6b6e…",
    "value": "ce17…"
  }
}
```

## 设计取舍

- **签名覆盖"去掉 signature 字段的清单字节"**，签的和验的是同一串字节（有测试钉住）。
- **不内置 `tempfile`/`getrandom`**：它们给 gnu 工具链带来 `dlltool` 依赖，而 32 字节随机数不值得
  为它换工具链——改用 `BCryptGenRandom` / `/dev/urandom`。
- **符号链接不进清单**：链接描述的是别的机器的磁盘。
- **清单文件本身不进清单**，所以 `manifest.json` 可以和它描述的文件放在一起。
- **多出来的文件算失败**：发布目录里不该有清单之外的东西。

## 检查更新：`o824 update --check`

只回答三个问题，**不下载、不替换**：

1. 这份清单是不是我信任的那把钥匙签的；
2. 它描述的版本是不是比当前新；
3. 如果拿它去更新，本地的哪些文件会变。

```bash
o824 update --check --manifest dist/manifest.json --public-key <hex> [--installed <dir>] [--json]
```

退出码：`0` 没有更新；`1` 有更新的版本（读命令的 "有发现"）；`2` 参数错；`4` 读不到文件；
`5` 清单不是那把钥匙签的。

**没有 `--apply`，而且不是"还没实现"，是故意不存在**：未签名二进制自更新需要先有"清单可信 +
钥匙内置 + 有人决定信任它"，那是决定，不是一个开关。

不给 `--public-key` 时它只会用清单里自带的钥匙验，输出会明说 `self-signed only: the key came
with the manifest`——那只能证明文件没被截断，证明不了是谁发的。这个区别写在屏幕上，不是藏在文档里。

## 内置公钥：让"信任谁"变成产品决定

`o824 update --check` 不给钥匙时，用**编进二进制的那把**：

```bash
O824_RELEASE_KEY=<公钥 hex> cargo build --release --bin o824
```

实测（2026-11-04）：

```
signature: ok (verified against the key built into this binary (091ac5a6625e321b…))
a newer release is published: 1 files, nothing downloaded        ← exit 1

# 换一把钥匙签的清单
manifest: it is signed by 5d00a50a23936fff, not by the expected key f89ed3222923ad20  ← exit 5
```

**没有内置钥匙是正常状态，不是降级**：开发构建不能为任何发布背书，所以它明说
`self-signed only`，而不是拿下载文件自带的钥匙冒充信任。

一个坑：`option_env!` 不会因为环境变量变化自动重编译——`crates/o824-manifest/build.rs`
里加了 `rerun-if-env-changed=O824_RELEASE_KEY`。没有它，"先构建无钥匙版、再构建有钥匙版"
会**静默留住无钥匙的二进制**，而这类静默错误正是可信钥匙最不该出的。

## CI 里为什么没有这一步

我曾加过一个 CI 步骤：带钥匙构建 → 用另一把钥匙签清单 → 断言退出码 5。它在 runner 上
**始终失败**，而且失败发生在脚本写出任何自己的诊断之前；抽出来的脚本本体在本地用
`pwsh -File` 跑是通的。诊断被两件事挡住：job 日志要管理员权限，checks API 只给注解。

所以它被摘掉了，不是为了绿而绿，而是因为它没有提供可靠信号。**信任解析规则本身有 5 个单元
测试**（显式钥匙优先 / 内置钥匙兜底 / 空参数穿透 / 没有钥匙则谁也不信 / 开发构建的措辞），
编译期注入则由下面这条手工验证覆盖。要把它做成 CI 门禁，需要先解决"怎么读到 runner 上的失败
原因"——那是工具问题，不是产品问题。

## 未签名说明：产品里的位置

§13 决定未签名分发，那么"用户第一次看到警告"就是产品的一部分，不是意外。现在它有三处落点：

1. **首屏右上角**：`Unofficial tool · unsigned · github.com/...` 旁边的链接打开一段说明——警告的含义
   （身份未知，不是有害）、怎么自己核对、以及**这个工具永远不会要求你关闭任何防护**；
2. **README**：同一段话的书面版本，带可复制的 `verify-manifest` 命令与三种结果（签名不对 / 文件被动 /
   都对）；
3. **文案测试**：`the_trust_help_never_asks_anyone_to_weaken_a_protection` 扫描两种语言的说明文本，
   出现 "disable/turn off/ignore/bypass" 或 "关闭/禁用/忽略/绕过" 就失败（"永远不要求你关闭"这句
   承诺本身除外，它被单独检查必须真的是承诺）。这条测一次之后，谁想加一句"直接点仍要运行就行"
   都会被拦下来。

截图见本次提交说明：面板在 `%LOCALAPPDATA%\o824` 的安装版上验证过。

## 发布脚本：一次命令产出可发布目录

```bash
scripts/release.sh 0.2.0-rc.1 release.key
```

它做五件事，任何一步不成立就停：

1. 用私钥文件**派生出公钥**（`o824 manifest-keygen --from`，0.2.0 新增）——发布页需要的是这把公钥；
2. 带 `O824_RELEASE_KEY=<公钥>` **重新构建内核**。`option_env!` 是编译期读取，暖缓存可能让构建
   不重编而留下无钥匙的二进制，所以先 `cargo clean -p o824-manifest` 再编；
3. 用同一把钥匙构建窗口（MSVC 工具链，复用 `env-msvc.sh`）；
4. 把两个 exe 收进 `dist/<版本>/`，生成清单并**签名**；
5. **用发布公钥验签 + 逐文件比对**，然后**自检发布版是否真的信任这把钥匙**——最后这条是必需的：
   第一次跑的时候我发现内核是在导出公钥之前构建的，于是发布版只认成 `self-signed only`。

实测输出（0.2.0-rc.1，一次性密钥）：

```
signature: ok (f027712ed36acd71)
files: ok (2 checked)

== does the released kernel trust its own key? ==
   ok: the binary carries the key
```

脚本**不上传**任何东西，只把"发布页需要写什么"打出来：版本、公钥、清单的 sha256，以及那段
未签名说明。上传是人的决定。


## 安装器：`o824 install` / `o824 uninstall`

不是打包格式，是"把两个 exe 放到它该在的地方"：

```
o824.exe install            # -> %LOCALAPPDATA%\o824，开始菜单快捷方式，卸载项（HKCU）
o824.exe uninstall --yes    # 三样一起删
```

- **不需要管理员**，也不请求；
- 装到 `%LOCALAPPDATA%` 是**功能要求**，不是偏好：工作区里的文件带 Low 完整性标签，从那儿运行
  的进程拿不到通知区域图标（见 `docs/TRAY-INVESTIGATION.md`）；
- 卸载项写在 `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\O824`；`UninstallString`
  带引号并指向 `o824.exe uninstall --yes`，所以路径里有空格也能用；`NoModify`/`NoRepair` 置 1——
  没有可修可改的东西，摆着按钮只会让人点了失败；
- 删除是破坏性操作，缺 `--yes` 就退 3（§20.7 的二次确认）。

实测（2026-11-05，从 `%LOCALAPPDATA%` 下的副本运行，不是在构建目录里）：

```
registry: DisplayName=O824  DisplayVersion=0.1.0-dev  UninstallString="...\o824.exe" uninstall --yes
uninstall 无 --yes -> exit 3
uninstall --yes    -> removed ...；注册表项与目录都消失
```

从**构建目录**里跑 install 时写注册表会得到 `ACCESS_DENIED`——同一条 Low 完整性规则。这不是 bug；
install 因此把那一步当警告而不是假装成功。


## 下载并校验：`o824 update --fetch`

```bash
o824.exe update --fetch --manifest-url <url>/manifest.json --base-url <url> --into <dir> [--public-key <hex>]
```

顺序是设计的一部分：**先下载清单、先验签，其它文件才开始下载**。所以一个恶意镜像发一份未签名的
清单，后面什么都不会跟着下来。之后每个文件都要与清单里的哈希和大小一致，每个路径都要**留在目标目录
内**——清单里写 `..`、绝对路径或盘符会被拒绝而不是照做（`safe_relative`，有 11 个反例测试）。

它**不替换任何东西**。

实测（2026-11-05，本地起一个裸 TCP 文件服务，真下载）：

```
fetching http://127.0.0.1:8765/manifest.json
manifest verified (daf123367451214d), 2 files
  o824-gui.exe
  o824.exe
staged in ...\staged: 2 files, every hash matches. Nothing was replaced.   ← exit 0

# 把被服务的一个文件改掉，清单不动
changed after download: o824.exe
update: the downloaded files do not match the manifest                      ← exit 5
```

### 两个实现坑

1. **urlmon 需要 COM**：`URLDownloadToFileW` 在线程没初始化 COM 时返回 `INET_E_DOWNLOAD_FAILURE`
   （`0x800C0008`），看起来像网络坏了。现在下载前后各调一次 `CoInitializeEx`/`CoUninitialize`，
   且只在"是这次调用初始化的"时才反初始化。
2. **必须从安装位置运行**：从工作区跑（Low IL）下载同样失败——和托盘是同一条规则。帮助文本里
   写明了这一点。


## 界面里的更新流程

托盘的「Check for updates…」不再打开浏览器，而是让窗口自己跑完整条链：

1. **检查**：窗口调用内核 `update --fetch`，把发布下载到 `%LOCALAPPDATA%\o824\staged` 并逐字节校验；
2. **显示证据**：面板写明「版本 A 可用 / 正在运行 B / 该发布有 N 个文件 /
   签名已用密钥 <keyId> 验证 / **尚未安装任何东西**」；
3. **安装**：只有按下 `Install <版本>` 才会调用 `update --apply --yes`；内核负责备份与失败回退，
   完成后窗口调用 `quit_now` 退出，让新装的程序接管那个唯一的窗口。

窗口**没有**重新实现校验：它调用的内核已经会拒绝签名或哈希不符的发布，再写一遍只会多一处出错的地方。

实测（2026-11-05，本地起发布服务，`O824_UPDATE_URL` 指向它）：

```
update: fetching http://127.0.0.1:8765 into ...\o824\staged
update: kernel exited 0
```

界面（截图见提交）：`Version 9.9.9 is available / Running 0.1.0-dev. The release has 2 files. /
Signature verified with key 5c3345b3e56301d3. Nothing has been installed.` + `[Not now] [Install 9.9.9]`。

发布地址默认是 GitHub 的 `releases/latest/download`，`O824_UPDATE_URL` 可覆盖——那正是验证时用的。

## 还没做的（P4 其余部分）

- 安装器；自更新的"检查—下载—校验—替换"；崩溃恢复。
- 公钥的分发与轮换策略（`keyId` 已留位）。
- CI 里"构建 → 签名 → 验签"需要私钥入库，属于发布流程决定，暂未接。
