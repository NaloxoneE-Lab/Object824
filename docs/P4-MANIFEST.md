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

## 还没做的（P4 其余部分）

- 安装器；自更新的"检查—下载—校验—替换"；崩溃恢复。
- 公钥的分发与轮换策略（`keyId` 已留位）。
- CI 里"构建 → 签名 → 验签"需要私钥入库，属于发布流程决定，暂未接。
