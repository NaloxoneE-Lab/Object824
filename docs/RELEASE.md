# 发布清单

每一步都写了"怎么知道它成了"。没写"应该没问题"这种话——这个项目今晚已经证明过，看起来对的东西可以整晚都是错的。

发布分两段：**准备一次**（生成密钥）和**每次发布**（构建、签名、上传、自查）。

---

## 0. 开始之前

| 需要 | 怎么确认 |
|---|---|
| 工具链在仓库内（`.toolchain`） | `source scripts/env.sh` 后 `cargo --version` 有输出 |
| MSVC 环境（窗口需要） | `source scripts/env-msvc.sh` 不报错；`echo $INCLUDE` 非空 |
| 一个 GitHub 账号能发 Release | 仓库→Releases→Draft a new release 能看到按钮 |

```bash
cd "D:\dsh work\Object 824\o824-rs"
source scripts/env.sh
source scripts/env-msvc.sh
```

---

## 1. 生成发布密钥（**只做一次，做完就锁起来**）

```bash
./target/release/o824.exe manifest-keygen --out release.key
```

输出两行：

```
secret key written to release.key
public key: <64 位十六进制>
```

**把那个公钥记下来**，它是这个项目以后所有发布的身份。私钥 `release.key`：

- **不要**放进仓库（`.gitignore` 里没有它，自己确认 `git status` 不显示它）；
- **不要**放进 `dist/`，`scripts/release.sh` 只读它；
- 换机器时只带私钥，公钥可以从私钥重新推出：
  ```bash
  ./target/release/o824.exe manifest-keygen --from release.key
  ```

> 私钥丢了 = 以后所有发布都不能再用同一把钥匙，用户的 `update` 会拒签。
> 私钥泄了 = 别人能签出"你的机器认为是官方发布"的包。这两件事都没有补救办法，只有预防。

---

## 2. 构建并签名（每次发布）

```bash
scripts/release.sh 0.2.0-rc.1 release.key
```

它按顺序做五件事，任何一步不成立就停：

1. 从私钥派生公钥；
2. 带 `O824_RELEASE_KEY=<公钥>` **重建内核**（先 `cargo clean -p o824-manifest`，避免暖缓存留下无钥匙的二进制）；
3. 用同一把钥匙构建窗口；
4. 把两个 exe 收进 `dist/<版本>/`，生成 `manifest.json` 并签名；
5. 用发布公钥验签 + 逐文件比对，**再自检发布版是否真的信任这把钥匙**。

成功时的输出长这样（0.2.0-rc.1 实测）：

```
signature: ok (f027712ed36acd71)
files: ok (2 checked)

== does the released kernel trust its own key? ==
   ok: the binary carries the key

   version        : 0.2.0-rc.1
   public key     : c536b65c…
   manifest sha256: 09be61ca…
```

**怎样算成了**：看到 `ok: the binary carries the key`。没有这一行就是没成——那说明发布版对自
己的清单只会说 `self-signed only`，用户下载后也会得到这个答案。

产出目录：

```
dist/0.2.0-rc.1/
    o824.exe
    o824-gui.exe
    manifest.json
```

---

## 3. 上传

GitHub → Releases → **Draft a new release**：

- **Tag**：`v<版本>`（例如 `v0.2.0-rc.1`）
- **Title**：`O824 <版本>`
- **Attach**：`dist/<版本>/` 里的**三个文件**，一个都不能少

> `manifest.json` 必须作为 **asset** 上传。`update` 默认从
> `https://github.com/Naloxone-E-Lab/Object824/releases/latest/download/manifest.json` 取，
> 这个地址只在资产真的挂在上传的 release 上时才解析得到。

---

## 4. 发布页上必须写的三件事

`scripts/release.sh` 会把它们打出来，直接抄：

1. **这是未签名程序**，第一次运行 Windows SmartScreen 会提示"无法识别的应用"——
   这个提示说的是**身份未知**，不是文件有害；
2. **怎么核对**：
   ```
   o824 verify-manifest --manifest manifest.json --root <解压出来的目录> --public-key <本轮公钥>
   ```
3. **本工具永远不会要求你关闭 SmartScreen、Defender 或任何其他防护**。如果有什么东西让你这么做，那不是我们。

再附上这一轮的 **manifest 的 sha256**（脚本会打印），有人愿意做二次核对时用得上。

---

## 5. 自查（当作陌生人，不要当成作者）

在**另一台机器或另一个目录**里，下载发布页上的三个文件，然后：

```powershell
# 1. 验签 + 逐文件比对
.\o824.exe verify-manifest --manifest manifest.json --root . --public-key <本轮公钥>
# 期望：signature: ok / files: ok (N checked)

# 2. 故意改一个字节，确认它会被拒
Add-Content .\o824-gui.exe "x"
.\o824.exe verify-manifest --manifest manifest.json --root . --public-key <本轮公钥>
# 期望：changed: o824-gui.exe，退出码 5

# 3. 用发布版问它自己的清单（不给 --public-key）
.\o824.exe update --check --manifest manifest.json --version 0.0.0
# 期望提到 "the key built into this binary"
```

**怎样算成了**：三条都对。第二条不对说明校验是假的，第三条不对说明内置公钥没编进去。

最后在装好的机器上让界面走一遍：托盘 → Check for updates → 应当显示
`Signature verified with key <keyId>. Nothing has been installed.`

> 如果界面里报的是"无法获取"，先确认这台机器能访问 GitHub；本机 HTTPS 偶尔不通时，
> 它给的就是这条错误，而不是假装有更新。

---

## 6. 一条退出路径（发布错了怎么办）

- 发布页上的旧版本**不要删**：用户可能正是从它升上来的；
- 严重问题时：把坏的那次 release 标记为 **pre-release**，再发一个修正版本（版本号更高）。
  `update` 只接受**更高**的版本，降级需要 `--force`，所以"发低版本让用户下回去"不会自动生效；
- 用户本地：每次 `update --apply` 都会把被替换的文件留成 `<名字>.bak`，需要手动退回时把它们
  改回来即可；彻底移除用 `o824 uninstall --yes`。

---

## 7. 一页检查表

发布前：

- [ ] `release.key` 不在仓库里（`git status --short` 看不到它）
- [ ] `scripts/release.sh` 跑完，看到 `ok: the binary carries the key`
- [ ] `dist/<版本>/` 里恰好三个文件：两个 exe + `manifest.json`
- [ ] 记下了本轮公钥和 `manifest.json` 的 sha256

发布时：

- [ ] tag 形如 `v<版本>`
- [ ] 三个 asset 都传了（缺 `manifest.json` 更新链就是断的）
- [ ] 发布页写了未签名说明、核对命令、"永不要求关闭防护"

发布后：

- [ ] 在干净目录里 `verify-manifest` 通过
- [ ] 改一个字节后 `verify-manifest` 被拒（退出码 5）
- [ ] 发布版 `update --check` 说 "the key built into this binary"
- [ ] 界面里 Check for updates 显示 `Nothing has been installed.`

---

## 8. 明确不做的

- **不**为通过 SmartScreen 去买证书、或者让用户去装自签根证书（2026-10-31 的决定，见设计文档 §13）；
- **不**让工具自己静默替换二进制：`update --apply` 必须由人按下按钮，且要 `--yes`；
- **不**在发布页上劝人忽略警告。警告是真的，只是它的含义是"身份未知"。
