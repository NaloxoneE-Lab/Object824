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

## 还没做的（P4 其余部分）

- 安装器；自更新的"检查—下载—校验—替换"；崩溃恢复。
- 公钥的分发与轮换策略（`keyId` 已留位）。
- CI 里"构建 → 签名 → 验签"需要私钥入库，属于发布流程决定，暂未接。
