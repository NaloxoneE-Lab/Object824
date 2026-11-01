# 05 · 第一棵 fixture 树与 asar 格式

目标：`o824-fixtures::build("ok")` 造出一棵能被 `o824-discovery::validate` 判为健康的树。它是后面**所有**测试的地基。

## 树的形状（八种 kind 共用这个骨架）

```
<root>\
├── DeepSeek Harness.exe          0 字节（存在即可，discovery 只检查是普通文件）
├── version                       "44.7.0\n"        ← Electron 版本，故意不是宿主版本
├── resources\
│   └── app.asar                  ← 真 asar，内含 package.json{"version":"0.2.1-rc.3"}
└── profiles\
    └── desktop\
        ├── package.json          ← dsh.profile.bundles 数组
        ├── cordis.yml            "[]\n"
        ├── cordis.patch.yml      YAML 顶层数组
        ├── pnpm-lock.yaml        "lockfileVersion: '9.0'\n"
        └── pnpm-workspace.yaml   ← 注意：在 profile 目录下，不在 root
```

**为什么 `version` 写 44.7.0**：真机就是这样——安装根的 `version` 是 Electron 的版本，宿主版本在 asar 的 `package.json` 里。fixture 必须保留这个陷阱，否则"从 version 文件读版本"这种错误实现会蒙混过关。

## asar 格式（照抄 Go 的写法，字节级）

```
偏移 0:  uint32 LE  = 4                    // 第一个 pickle 的长度字段
偏移 4:  uint32 LE  = len(headerPickle)
偏移 8:  headerPickle                      // 见下
之后:    package.json 的内容（无压缩）

headerPickle:
  [0:4]   uint32 LE = payloadSize = 4 + len(headerJson) + padding
  [4:8]   uint32 LE = len(headerJson)
  [8:8+len(headerJson)]  headerJson（UTF-8）
  padding = (4 - len(headerJson) % 4) % 4 个 0 字节

headerJson = {"files":{"package.json":{"size":N,"offset":"0"}}}
package.json 内容 = {"name":"deepseek-harness","version":"0.2.1-rc.3"}
```

**读取端的定位规则**：文件内容从 `8 + headerPickleLen + offset` 开始，其中 `headerPickleLen` 是偏移 4 那个 uint32。这条规则 Go 版和 Rust 版必须一致，否则 fixture 造出来的树在两边行为不同。

一个更省事的替代：直接把 Go 仓库里 `internal/fixtures/fixtures.go` 的 `writeAsar` 逐行翻译（40 行），不要自己发明格式。

## 八种 kind 的差异

| kind | 与 `ok` 的差别 |
|---|---|
| `ok` | 无。**必须干净**：七个检查一条 finding 都不许有 |
| `broken-link` | `node_modules/@deepseek-ai/dsh-base` 是指向 `<root>/vendor/dsh-base-empty` 的 junction；目标**存在但没有 package.json** → F01 |
| `missing-bundle` | 同上的 junction，但目标目录**已被删除** → F02 |
| `dup-loader` | `cordis.patch.yml` 里有一行 id 与 bundles 最后一段同名 → F03（warning） |
| `parse-error` | `cordis.yml` 是坏的 YAML（如 `[\n  broken: [\n`）→ F16，带行号 |
| `half-install` | **没有 asar**；另在 data root 的 `journal/install-1.json` 里写 `{"state":"applying"}` → F20 |
| `dangling-bridge` | profile 的 `package.json` 有 `link:` 依赖，对应 `<plugin>/node_modules/@deepseek-ai/<pkg>` 桥缺失 → F04 |
| `git-dep` | `package.json` 有一个 `git+https://…` 依赖，`pnpm-workspace.yaml` 的 `allowBuilds` map 里没有它 → F17 |

**`ok` 的硬要求**：它同时是"零 finding"的基准。任何一条检查在 `ok` 上报了东西，就是那条检查的实现错了（或者 fixture 造错了），**不要改 fixture 迁就实现**。

## junction / symlink

`broken-link`、`missing-bundle`、`dangling-bridge` 需要链接。要求：

- 优先 `std::os::windows::fs::symlink_dir`；
- 失败则退到 `cmd /c mklink /J`（**argv 形式，不是拼字符串**——路径里可能有 `&`/`%`）；
- 两者都失败则 `panic!`（不要静默跳过：跳过会让 F01/F02 的测试变成假绿）。

`missing-bundle` 的特殊之处：junction 不能指向不存在的目标，所以做法是"先建目标目录 → 建 junction → 删掉目标目录"。

## 完成判据

- [ ] `build("ok")` 造出的树，`discovery::validate` 返回 `health == "ok"`、`version == "0.2.1-rc.3"`。
- [ ] 八种 kind 都能造出来，且 `half-install` 没有 asar。
- [ ] 在真机上（Windows）跑，链接是 junction 或 symlink，不是复制出来的目录。
- [ ] 造树的耗时 < 0.5 秒（后面几百个测试都要调它）。

## 下一步

树造出来之后，第 2 周的第一件事是 `verify`——**先让 8 个 `.doctor.json` 全绿**，再谈别的。
