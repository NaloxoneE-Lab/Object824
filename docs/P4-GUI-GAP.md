# P4 GUI 还原差距清单（对照鹰角启动器 1.6.0）

> 参考物：`E:\Program Files\Hypergryph Launcher\1.6.0`（Qt 5 + WebEngine，UI 在 `res/web`）。
> 本文只做**差距分析**，不改代码。数值出处两类：
> - 〔CSS〕= `res/web/index.ac90f4.css` 原文（624 条规则，Tailwind v3.4.17 编译产物）；
> - 〔JS〕= `res/web/index.ff0a17.js`（990,966 字符，client@0.2.15）里的 JSX className / 行内 style，已按 `1rem=16px` 折算。
>
> 我们这边：`apps/o824-gui/dist/{index.html,style.css,app.js}`、`src/main.rs`。
>
> **状态**：P0 五项已于 2026-11-05 落地，落地内容见 §13。§1–§8 里「我们」一列写的是**落地前**的状态。

## 0. 三条会影响结论的前提

1. **启动器的窗口骨架是「两块浮卡 + 12px 透明边」，不是一整块圆角矩形。**〔JS〕根组件：
   `flex h-screen bg-white/0 rounded-3xl p-3 gap-2` → `p-3`=12px、`gap-2`=8px、`rounded-3xl`=24px、根背景**全透明、无阴影**；
   左栏 `w-[88px] … rounded-[18px] border-[1px] border-white/10 bg-[rgb(24,23,26)]/90 shadow 0 2px 8px rgba(0,0,0,.45)`；
   主面板 `flex-1 rounded-[18px] overflow-hidden shadow 0 2px 8px rgba(0,0,0,.45)`，内部再叠 `inset-0 border-[1px] border-white/10 rounded-[18px]` 与 `inset-[1px] rounded-[17px] bg-[rgb(24,23,26)]/90`。
   我们：`.app` 整块 `border-radius:16px`、背景 `--frame #a5a5a5` 不透明、rail/stage 各 16px、无边框无阴影。
   ⚠️ 参考图（1583×869）看起来把 12px 透明边裁掉了（右侧仍能看到 12px 阴影带、左侧没有），所以改这条之前建议先补拍一张**含完整窗口边界**的截图确认。
2. **设置 / 登录 / 游戏设置 / 客服 / 个人中心是 Qt 原生对话框**，不在 web 包里。`Games.exe` 里有 `dialog_name`、`launcher_setting`、`game_setting`、`login_dialog`；JS 只发 `open_dialog`。全库 `代理/缓存/日志/分辨率/画质/语言` 命中 0，`version` 命中 0。
   → 我们的设置弹层没有可对照的原版，属于自创。
3. **主按钮文案不写在 JS 里**，由原生 `main_btn_state_rsp {state, text_normal, text_hover, tooltip, sub_tip}` 下发；JS 只有 19 个状态枚举与每个状态的图标/底盘/箭头动画。

---

## 1. 窗口与外壳

| 项 | 启动器 | 我们 | 差距 |
|---|---|---|---|
| 根容器 | `p-3`(12) + `gap-2`(8) + `rounded-3xl`(24)，透明、无阴影〔JS〕 | `.app` grid 88+8，`radius:16`，背景 `#a5a5a5` 不透明〔style.css:72〕 | 少 12px 透明边；圆角 16 vs 24；底是不透明灰 |
| 左栏 | 88 宽，`radius:18`，`bg rgb(24,23,26)/90`，`1px white/10` 描边，`0 2px 8px .45` 阴影，`py:20`〔JS〕 | 88 宽，`radius:16`，`linear-gradient(#1d1c1e,#19181a)`，无描边无阴影，`padding-top:30`〔86-96〕 | 圆角/描边/阴影/内边距全不同 |
| 主面板 | `radius:18` + `inset-0` 1px 白 10% 内描边 + `inset-[1px]` 深色底 + 阴影 | `.stage` `radius:16`，`--stage #c4c4c4` 兜底，无描边无阴影〔270〕 | 缺内描边与阴影 |
| 窗口动作 | 原生：最小化/关闭（无最大化按钮） | 同（minimize/hide/quit） | — |
| 可见内容宽 | 1242（1266−24） | 1266 | 我们的栏/面板比对方宽 12px |

## 2. 左侧游戏栏

| 项 | 启动器 | 我们 | 差距 |
|---|---|---|---|
| 图标外框 | 48 图 + `0.5px` 描边 + `p-[3px]` + `2px` 环 = **59×59**；外圆角 12、图圆角 8；`0 2px 6px .55` 阴影；`border-white/10`〔JS〕 | 48×48，圆角 6，`::after` inset −6 做 60 环〔139-170〕 | 圆角 6 vs 12/8；无 0.5px 描边；无阴影 |
| 选中态 | 2px 边框，色 `rgba(255,255,255,.75)`，`transition .3s`（新装延迟 1.6s）〔JS〕 | `::after` 2px `#f2f2f3` 实色〔CSS〕 | 75% 白 vs 实色 |
| hover | 环 `rgba(255,255,255,.35)` + 图标 `scale(1.08)`〔CSS〕 | 环 28% 白，无缩放 | 缺缩放 |
| active | `scale(.97)`〔CSS〕 | 无 | 缺按下态 |
| 拖拽排序 | dnd-kit，8px 起拖、Y 轴夹取、落位 `sorting-shake .25s`、写回 `game_list_update_req`〔JS〕 | 无 | 整块缺失 |
| 下载状态 | 图标上盖 48×48 深色遮罩 + `r=16 stroke=4` 进度环 + 队列/暂停/异常图标〔JS〕 | 11×11 角点（`.badge`） | 形态完全不同 |
| hover 提示 | 右侧 tooltip，`offset 12`，max 360px〔JS〕 | 原生 `title` | 缺 tooltip 组件 |
| 间距 | `space-y-2`=8px，pitch 67 | `gap:19`，pitch 67（48+19） | 数值巧合一致，但构成不同 |
| 顶部 logo | SVG `gf`，54×48，`mb-20`〔JS〕 | `logo.png` 839B 位图，45 宽〔105-110〕 | 位图放大发虚；位置偏低 |
| 底部按钮 | 52×52，2×2 六个白点，选中整块 `rotate(45deg)`；tooltip 全部游戏/返回〔JS〕 | 52×52，2px 边框，`scaleX(1.4)` / 打开时 45°〔204-240〕 | 边框与 idle 形变需复核 |

## 3. 顶栏

| 项 | 启动器 | 我们 | 差距 |
|---|---|---|---|
| 位置 | `fixed top-3 right-3 p-4` → 内容距顶/右各 **28**〔JS〕 | `top:16; right:28`〔290〕 | 顶部差 12px |
| 分组 | 左组 `gap-4`(16)、右组 `gap-3`(12)，中间 `1px×24` 白 35% 分隔线 `mx-4`〔JS〕 | 统一 `gap:16`，无分隔线 | 缺分隔线；右组偏宽 |
| 圆钮 | `2.5rem` + `0.5px` 描边，`box-sizing:initial` → **41×41**；`bg #18171af2`；hover 叠 15% 白；**无 transition**〔CSS〕 | 40×40（border-box），hover 15% 白，`transition 120ms` | 1px 尺寸；多过渡 |
| 登录胶囊 | 高 41，`padding 0 12`，13px/18，`gap-4`，图标 24，文字 `max-w-116 truncate`〔CSS/JS〕 | chip 高 40，同 padding/字号，`margin-right:20` | 内容不同（我们是「检查更新」） |
| 图标 | 人形 24、齿轮 16、最小化 14（2px 线）、关闭 24〔JS〕 | 同（已复用） | — |
| 缺的 | 消息中心铃铛 15×15（`yf`）、充值入口 24（`If`） | 无 | 结构缺失 |
| 多的 | — | 7px 更新红点 `.dot` | 自创 |

## 4. 主按钮 CTA

| 项 | 启动器 | 我们 | 差距 |
|---|---|---|---|
| 尺寸 | 220×60 单一元素，`rounded-100`，`shadow 0 3px 10px .55`〔JS〕 | `.cta-shell` 220×60（黄底）+ `padding:4` + `.cta` 高 52〔482-511〕 | 可见尺寸一致；内层点击区 52 高、图标位 12 vs 16 |
| 文字 | 16px/700 `#18171A` 居中于剩余空间；溢出自动 14px、两行截断、>22px 左对齐〔JS〕 | 16px/700，`padding-left:12` + `gap:9`，无自适应 | 缺 14px 回退/两行/左对齐 |
| 图标位 | `pl-[16px] pr-[9px]`，26×26 底盘：Solid=`rect rx12 stroke2` / Progress=`circle r12` 进度环〔JS〕 | `padding-left:12`，26×26 `border:2px` 自绘圆环〔527-541〕 | padding 差 4px；底盘不是 launcher 的 rect |
| 菜单圆钮 | 52×52，`rgba(24,23,26,.78)`，`mr-1`(4)，内部 18×12 三根 2px 白条〔JS〕 | 52×52，`#4b4914`，图标 18×12〔559-580〕 | 颜色一致（#4b4914 就是 78% 黑叠在黄底上的合成值），差的是它由「半透明+透底」实现，我们是实色 |
| 汉堡→✕ | `y:±5 / rotate:±45 / 中间 opacity:0`，`.1s`〔JS〕 | 直接换 SVG（18×12 ↔ 24×24），无动画 | 缺动画 |
| hover | 背景 `rgb(254 253 77)`〔CSS〕 | `filter: brightness(1.04)`〔513〕 | 色值不同 |
| 按下 | `rgb(228 227 0)`〔CSS〕 | 无 | 缺 |
| 禁用 | `rgb(160 160 160)` 灰〔CSS〕 | `.is-idle` `opacity:.8`〔493〕 | 形态不同 |
| hover 图标/文案 | 换 `text_hover` + hover 图标，并有 tooltip〔JS〕 | 无 | 缺 |
| 复选框 | 16×16 `r4`，无边框，白 25/55/75/100% 四态，黑勾 `stroke 1.818`；`top:calc(100%+10px)`、`right:-8px`、12px/14、双层 text-shadow〔JS〕 | 14×14 `r3`，1px 白 85% 边框，白 35% 底〔593-620〕 | 尺寸/圆角/四态/边框全不同 |
| 进度 | 斜纹条 `h-2 r-18`、3×10 斜纹、`bg-primary` 填充 + 外发光；速度/剩余时间卡片 320 宽〔JS〕 | 无 | 整块缺失 |

## 5. ≡ 菜单

| 项 | 启动器 | 我们 | 差距 |
|---|---|---|---|
| 面板 | 220 宽，`p-2`(8)，`r8`，`bg rgb(24,23,26)/.9`，`0.5px white/10`，`blur 4`，`shadow 0 2px 8px .65`〔JS〕 | 220 宽，`padding:6`，`r10`，`#232326` 实色，无描边/模糊，`shadow 0 22px 48px .36`〔616-628〕 | 圆角/底/模糊/阴影全不同 |
| 菜单项 | 高 42（18 行高 + `py-3`×2），14px/18，`r4`，`px-2`，hover `white/15`〔JS〕 | `padding:10px 14px`，13px，`r7`，hover `#2f2f34`〔630-646〕 | 字号/圆角/高度/悬停色 |
| 位置 | `top-right` `offset:8`〔JS〕 | `bottom:calc(100%+20px)`〔618〕 | 间距 20 vs 8 |
| 出现 | `opacity 0→1` `.3s`〔JS〕 | 无动画 | 缺 |

## 6. 弹层（我们自创，但可对齐）

| 项 | 启动器通用 modal | 我们 | 差距 |
|---|---|---|---|
| 遮罩 | `bg-black/40` + `blur 6` + `rounded-3xl`，`.2s` | `rgba(9,9,11,.34)` + `blur 8`，无圆角，`.12s` | 浓度/模糊/圆角/时长 |
| 面板 | 470 宽、`p-24`、`r16`、`bg #18171A`、`1px white/10`、`0 2px 8px .65`、`gap 24`、`scale .95→1` `.2s` | 740 宽、`r14`、`shadow 0 34px 90px .5`、无 scale | 尺寸/圆角/阴影/动画 |
| 关闭钮 | 16px 线 1.5，`top-4 right-4`，白 50→80% | 30×30 按钮 + 24 图标 + hover 底 | 形态不同 |
| 标题 | 16px/500 居中 | 14px/600 左对齐 + 图标 | 不同 |

## 7. 字体

| 项 | 启动器 | 我们 |
|---|---|---|
| 全局 | `html{font-family:inherit}`，**未声明**，实际走 WebEngine 默认（中文 Windows = Microsoft YaHei）〔CSS〕 | `"Segoe UI Variable Text","Segoe UI","Microsoft YaHei UI",…`〔22-23〕 |
| 局部 | `Source Han Sans CN`（pop 弹层 3 处）、`DFKai-sb`（繁中）、`Novecento wide`（百分比） | 无 |
| 影响 | 字形/字宽不同 → 所有换行位置漂 1–3px/行 | — |

## 8. 动效

启动器有 10 个 keyframes：`loading`、`arrowLoopAnimating`(2s/延迟1s)、`arrowAnimating`(.8s)、`activeFade`/`prevFade`(.4s，1.027 缩放)、`fadeSlideIn`(.2s)/`fadeSlideOut`(.1s)、`survey-ripple`(1s)、`sorting-shake`(.25s)、`spin`(3s 渐变描边)。
我们只有 `rise`(.14s)/`fade`(.12s)/`lift`(.14s)/`spin`(.8s)，另加 `prefers-reduced-motion` 兜底（对方没有）。
另：启动器有整页切换 300ms 过渡（`content-container` / `immersive-container`），我们没有；启动器全站有 `:active`，我们没有。

## 9. 状态机（最大的一块）

- 主按钮 **19 态**：`continue_download / download_game / downloading / checking_download_zip / checking_game_complete / installing / download_except / download_paused / queuing_download / start_game / update_game / game_service_stopped / in_game / need_login / need_purchase / prepare_start_game / unbook / booked / disable_download`。
- 每态绑定：底盘（Solid/Progress）、图标（`uf` 下载箭头 / `wf` 暂停 / `ff` 感叹号 / `Of` 播放 / `Cf+mf` 加载 / `Sf` 时钟）、箭头动画（Click/Loop）、是否禁用。
- 预下载独立 60×60 圆钮（9 态）。
- 我们：4 个 action（start/repair/install/choose）+ busy 文案，无进度环、无 hover 切换、无 tooltip。

## 10. 启动器有、我们没有的界面

| 界面 | 关键规格 | 我们的现状 |
|---|---|---|
| 全部游戏（cover-flow） | 封面 162×162，未选 `scale .8025`，间距 `15n+130n+(i>sel?32:0)`，选中 `3px` 白描边，`fadeSlideIn .2s` | 「全部安装」列表弹层（`#sheet-installs`） |
| 404 暂无游戏 | 220×140 SVG 插画 + 「暂无游戏」 | 空机器 = 灰图标 + 文案 |
| 公告区 | 左下 146px 高：横幅 260 宽（5s 轮播）+ 列表 420 宽 + Tab 指示条 3px `bg-primary` | 已补：两 tab 列表 + 标题卡（见 §14） |
| 右侧工具栏 | 56 宽，`rounded-l-16`，`bg-black/30` + `blur 6`，数据面板/问卷（红点脉冲）/运营位 | 已补：社区四入口（见 §14） |
| 消息中心 | 铃铛 + popover 365 宽（设备升级提醒，含 CPU/BIOS 表） | 无（我们是更新胶囊） |
| 背景切换 | 顶部 180×80 热区 → 108×32 玻璃胶囊，视频/静态双 tab | 无 |
| 沉浸模式 | 双击空白进入、右键退出，300ms 位移过渡 | 无 |

## 11. 我们多出来的（不算未还原，但要注意）

左下「桌面版 / DeepSeek Harness 0.2.1-rc.3 / 可以启动」信息块、更新胶囊、详情/修复计划/安装引导/信任说明/关闭确认等 8 个 sheet —— 启动器主界面没有对应物。这些是我们的信息设计，保持即可，但它们是和参考图肉眼差异最大的一处。

## 12. 建议顺序

**P0（数值级，半天）**
1. 顶栏：`top:28`、右组 `gap:12`、加 1px×24 白 35% 分隔线；
2. CTA：去掉 shell 的 4px padding（黄色区 220×60），菜单圆钮底色改 `rgba(24,23,26,.78)`，hover/pressed/disabled 改三色 `#FEFD4D / #E4E300 / #A0A0A0`；
3. 菜单：`r8`、`p-2`、项高 42/14px、`r4`、hover 白 15%、底 `rgb(24,23,26)/.9`+`blur 4`+`0.5px` 描边、offset 8；
4. 复选框 16×16 `r4` 无边框四态；
5. logo 换矢量/256px 源（`res/icons/app.ico` 或 SVG `gf`）。

**P1（动效/手感，1–2 天）**
6. 图标 hover `scale(1.08)` / active `.97`、选中环 75% 白；
7. 汉堡→✕ 的 ±45°/±5px 动画；
8. 菜单/弹层出现动画（opacity .3s / scale .95 .2s）；
9. 字体栈改 `"Microsoft YaHei", …`（或打包思源）；
10. 滚动条 4px、thumb 白 25%。

**P2（结构级）**
11. 窗口骨架（12px 透明边 + 18/24 圆角 + 描边 + 阴影）——需先确认截图口径；
12. 拖拽排序；13. 下载状态机（进度环/速度/剩余时间）；14. 全部游戏页；15. 公告/右侧栏。

**不建议照抄**：登录/个人中心/充值/消息中心/微码提醒（我们是开发者启动器，无账号体系）；预下载、商店跳转、沉浸模式。

---

## 13. P0 落地记录（2026-11-05）

改了 `dist/style.css`、`dist/index.html`，新增 `dist/assets/logo.svg`；重新编译 `o824-gui.exe` 后按 1477→1585×870 截图核对。

| # | 项 | 改法 |
|---|---|---|
| 1 | 顶栏 | `.topbar` 改 `top:16 / right:16`（= 启动器 28 − 它自己的 12px 窗口内缩，我们还没有那层内缩）；新增 `.topbar-group{gap:12}` 与 `.topbar-sep{1×24, rgba(255,255,255,.35)}`；删掉 `.chip{margin-right:20}`。HTML 拆成「账号组 / 分隔线 / 窗口组」三段 |
| 2 | 主按钮三态 | `:root` 增 `--yellow-hover #FEFD4D`、`--yellow-pressed #E4E300`、`--yellow-disabled #A0A0A0`、`--yellow-disabled-hover #888`；`.cta-shell:has(#primary:not(:disabled):hover/active)` 换底色（指针在菜单圆钮上不点亮药丸）；`.is-idle` 由 `opacity:.8` 改成灰底；删掉 `filter:brightness(1.04)` |
| 3 | ≡ 菜单 | `r10→r8`、`padding 6→8`、`bottom calc(100%+20)→+8`、底 `#232326→rgba(24,23,26,.9)`、加 `0.5px` 白 10% 描边 + `blur 4`、阴影 `0 22px 48px .36→0 2px 8px .65`；项 `padding 10/14→12/8`（行高 42）、`13px/500→14px/400`、`r7→r4`、hover `#2f2f34→white/15`、禁用 `#77777f→white/35`；分隔线 1px→0.5px |
| 4 | 复选框 | `14×14/r3/1px 边框/白 35%` → `16×16/r4/无边框`，四态 `白 25% / hover 55% / 选中 75% / 选中 hover 100%`，勾改用启动器的 16×16 path（`M6.26118 11.6228…`，`stroke-width 1.81818`）；`.checkline.dark` 显式保留 1px 边框，避免被基础规则清掉 |
| 5 | logo | 从 `index.ff0a17.js` 的 `gf` 组件抄出两条 path，存 `assets/logo.svg`（`viewBox 0 0 54 48`）；`.rail-logo` 48×34→54×48、栏顶内边距 30→20（`py-[1.25rem]`）、去掉 `opacity .96`；关于页 `about-logo` 58×52。原 `logo.png`（61×42 位图）不再被引用 |

附带修掉一个只有打开菜单才看得见的瑕疵：`.menu-toggle[aria-expanded="true"] .ico` 原来沿用 18×12 的盒子，把 24×24 的 ✕ 压扁成 12px 宽；现在开态给 24×24。

**没做的**（留给 P1/P2）：`.chip/.round` 的 41px（content-box）与去掉 120ms 过渡、汉堡→✕ 的 ±45°/±5px 动画、菜单出现动画、字体栈、滚动条、窗口骨架、图标 hover/active、拖拽排序、下载状态机。

**核对用的截图**：`.review-scratch/gui-restyle/p0-main.png`（主界面）、`p0-menu.png`（菜单）、`p0-close.png`（关闭确认，验证深色复选框）、`p0-about.png`（关于，验证矢量 logo）、`p0-compare.png`（上参考 / 下当前）。

---

## 14. 第二批：充值中心、社区栏、公告栏（2026-11-05）

启动器上另外三块家具，这次按它的几何补上，内容换成本产品自己的。

| 面 | 启动器规格 | 落地 |
|---|---|---|
| 充值中心 | 右上左组第二颗 40px 圆钮，图标 `If`（24px 钱包/窗口） | 同位置同图标，点击 `open_url("https://platform.deepseek.com/top_up")` |
| 社区栏 | `w-[56px] py-[12px] gap-[10px] rounded-l-[16px] border-white/10 bg-black/30 backdrop-blur-6`，圆钮 30px、hover `bg-white/25` + `0 2px 6px .55` | 同几何（32px 钮 / 18px 线图标）；入口由 `app.js` 的 `COMMUNITY` 数组驱动：文档 / 仓库 / 社区（Discord）/ 邮箱，**换链接只改这一行数组** |
| 公告栏 | 左下 146px 卡片，260 横幅 + 420 列表，`rounded-16`，`0 2px 8px .65`，tab 13px/700 + 3px 主色下划线，列表底部渐隐 | 同尺寸；左卡=当前 tab 最新一条（kicker + 标题 + 日期），右列=其余行（日期 + 标题，点击开原页）；列表隐藏滚动条 |

**数据路径**（这是本次唯一的架构改动）：`api-docs.deepseek.com` 无 CORS 头，WebView 的 `fetch()` 会被拒，所以：

- 新增 `apps/o824-gui/src/news.rs`：抓 `/zh-cn/news/news261017` 的 HTML（侧边栏 `menu__link` → 标题+日期，按 href 去重）与 `/zh-cn/updates.md`（`## 时间:` / `###` 配对）；
- 新增 `news` 命令（`async` + `spawn_blocking`，不阻塞 UI），结果缓存为 `o824-news.json`，TTL 30 分钟，路径按 `%LOCALAPPDATA%\O824` → exe 目录 → `%TEMP%` 逐个探测可写；
- 失败时返回缓存并置 `stale`，全空时整块隐藏；
- 依赖新增 `ureq`（rustls）：`Cargo.toml` 里第一个 HTTP 客户端；二进制 9.6 → ~10MB；
- 本机实测：首次 306ms，命中缓存 0ms。

**布局代价**：主文案从 `left:44 / bottom:120` 改为 `left:40 / bottom:200`（给卡片让位，并与卡片左边线对齐）。

**测试**：`cargo test`（apps/o824-gui）5 个解析用例；`cargo test -p o824-present` 11 个文案用例全绿（新增 7 个 chrome key 双语齐全）。

---

## 15. P1 动效与手感（2026-11-06）

P1 剩下的五项，全部是"看着差不多、动起来不一样"的地方：

| # | 项 | 启动器 | 落地 |
|---|---|---|---|
| 1 | 左栏图标 | `.game-icon-interactive` `transition: transform .3s ease-in-out`，hover `scale(1.08)`、active `scale(.97)`；环色 hover 白 35%、选中白 75% | `.tab` 同参数（`transition 300ms`、`scale(1.08)/(.97)`、`will-change`），`::after` 环色改 `rgba(255,255,255,.35/.75)`，过渡 120→300ms |
| 2 | 汉堡 → ✕ | 三根 18×2px 白条，开态：上条 `y:5 rotate:45`、下条 `y:-5 rotate:-45`、中条 `opacity:0`，`duration:.1` | 换成三个 `<i>` 的 `.menu-bars`，由 `aria-expanded` 驱动同样的 transform/opacity，`100ms`；删掉原来"换 SVG"的 JS |
| 3 | 菜单 / 弹层出现 | 下拉 `opacity 0→1 .3s`；模态 `opacity 0→1 + scale .95→1 .2s`；遮罩 `.2s` | `.menu:not([hidden])` 加 `menu-in .3s`；`.sheet` 的 `lift` 改 `scale(.95)` + `200ms`；`.overlay` 的 `fade` 改 `200ms` |
| 4 | 字体栈 | `html{font-family:inherit}` → WebEngine 默认，中文 Windows 就是微软雅黑 | `--font` 改为 `"Microsoft YaHei UI","Microsoft YaHei","Segoe UI",…`；副作用：雅黑比 Segoe 宽，公告头条会折行 → `.notice-headline` 加 `text-wrap: balance` |
| 5 | 滚动条 | 4px、滑块白 25%、圆角 3、hover 50%、轨道上下各留 10px | 同值（原来 10px / 18% / 999px / 带 3px 透明边） |

**附带**（P0 遗留）：`.chip/.round` 改 `box-sizing: content-box` → 实际 41×41（和启动器的 `box-sizing:initial` 一致），并去掉 120ms 的背景过渡（启动器是瞬时切换）。

**没做的**：发行版切换的整页过渡（启动器 `content-container` 300ms 位移），因为我们的内容更新是同步重绘，做过渡要引入 out-in 时序；留在 P2 和拖拽排序一起考虑。

**验证方式**：开态截图确认 ✕ 由两根旋转条构成（`p2-menu.png`）；hover/active 是纯 CSS transform，没有合成鼠标可用（见 2026-11-04 的误点教训），按代码核对而非截图。

---

## 16. P2 结构（2026-11-06）

P2 四项里，两项做了、一项不适用、一项暂缓。

### 16.1 窗口骨架（做）

按 web 包的根组件落地，参考图的裁切争议按"以源码为准"处理（用户拍板）：

| 项 | 启动器 | 落地 |
|---|---|---|
| 根容器 | `h-screen rounded-3xl p-3 gap-2 bg-white/0`（24px 圆角、12px 内边距、8px 缝、**全透明无阴影**） | `.app` 同值；删掉原来的 `--frame` 不透明底 |
| 左栏 | `rounded-[18px] border-white/10 bg-[rgb(24,23,26)]/90 shadow 0 2px 8px .45` | 同值（原来是 16px 圆角 + 渐变、无描边无阴影） |
| 主面板 | `rounded-[18px] shadow .45` + 内层 `inset-0 border-white/10` + `inset-[1px] bg-[rgb(24,23,26)]/90` | 18px 圆角 + 同阴影 + `.stage::after` 的 1px 白 10% 内描边；**兜底底色仍是浅灰 `--stage`**（启动器是深色，但我们的正文是深色字，深底会读不出来） |
| 页脚 | `pl-10 pr-[60px] pb-10`：主按钮距面板右 60、药丸下沿距面板底 40；复选框浮在药丸下 10px | `.launch` 改 `right:60 / bottom:14 / gap:10`（块底是复选框，14+16+10=40 对上药丸下沿） |
| 公告卡 | 与主按钮同一底边 | `.notice` 改 `bottom:40` |
| 主文案 | —（启动器没有这块） | `bottom:200 → 210`，让开公告卡 |

**为什么敢改**：真启动器以管理员权限运行，非提权进程连它的窗口都恢复不了（`ShowWindow` 被 UIPI 拒），量不到；web 包是唯一可验证的来源，用户选了"按 web 包"。

**副作用**：窗口四周真的透明了，截图会带上桌面（我们的截图环境后面正好压着一个控制台）。为此加了截图钩子 `?backdrop=%23e6e6e6`，只在截图时给 `.app` 涂底色。

### 16.2 左栏拖拽排序（做）

- Pointer Events（不用 HTML5 DnD：48px 的块要拖影，而且 pointer capture 能让手势在指针离开块时继续）；
- **8px 起拖**（启动器 dnd-kit 的 `activationConstraint`），拖动时 `.is-dragging` 关掉过渡、直接跟手；
- 落位判定用**清除 transform 之前**的 `getBoundingClientRect`（这里踩过一个真 bug：先清 transform 再量，位置永远是原槽位，排序永远不动）；
- 落位后播 `.25s` 的 `sorting-shake`，顺序写 `localStorage["o824.order.v1"]`，启动时校验是一份合法排列才采用；
- 拖完那一下的 `click` 会落回原块，用 `state.suppressClick` 吃掉。

**验证**：加 `?dragtest=1` 钩子，在页面里派发 pointerdown/move/up（**不碰系统鼠标**），截图可见轨道顺序从 `desktop,web,tui` 变成 `web,desktop,tui`；该钩子不写 localStorage，测完不留痕。

### 16.3 切发行版的入场（做）

`select()` 换 kind 时给 `.hero-copy` 重放 `hero-arrive`（opacity 0→1 + 12px 上浮，300ms）。只做入场：内容是同步替换的，没有出场可播。启动器是 out-in 的 300ms 位移，差异只在"旧内容淡出"这一半。

### 16.4 下载状态机（不做，不适用）

启动器的 19 态主按钮是围绕**下载器**转的：`download_game / downloading / checking_download_zip / installing / download_paused / queuing_download …`，配进度环、速度、剩余时间。O824 的内核不下载 DSH——`install` 只是打开官方安装引导，`update` 是"下载发布包 → 校验签名 → 替换"的一次性调用，没有进度事件。硬做就是假进度条，故不做。

### 16.5 全部游戏页（暂缓）

启动器那是竖向 cover-flow（162px 封面、未选 `scale .8025`、选中 3px 白边），因为它的游戏库可以有很多款。我们只有三个固定发行版且已经全在左栏里，"全部游戏"页没有内容可放。真要做得先有"更多发行版"的产品决定。
