# typors Windows 安装包（Inno Setup）

> 角色：打包子代理。本文只描述**已经跑过真实命令**的行为，未验证的部分单独标注。
> 产物：`packaging\dist\typors-<version>-setup.exe`

**一句话结论**：安装器支持**自选安装目录**、**「仅我 / 所有用户」二选一**、**简体中文界面（保留英文可选）**、以及**把 typors 设为 `.md` 默认打开程序**（可取消、可卸载清理）。除「所有用户」模式需要 UAC 无法在本环境验证外，其余均端到端验证过。

---

## 0 · 工具链

| 项 | 值 |
| --- | --- |
| Inno Setup 版本 | **6.7.3**（`Compiler engine version: Inno Setup 6.7.3`） |
| 安装方式 | `winget install --id JRSoftware.InnoSetup -e --scope user`（**用户级**，无需管理员） |
| `ISCC.exe` | `C:\Users\chenzhi\AppData\Local\Programs\Inno Setup 6\ISCC.exe` |
| 安装脚本 | `packaging\typors.iss`（**UTF-8 with BOM**，含中文，见 §2） |
| 构建脚本 | `packaging\build-installer.ps1`（Windows PowerShell 5.1 及以上） |
| 语言文件 | `packaging\languages\ChineseSimplified.isl`（**UTF-8 无 BOM**，见 §2） |
| `signtool.exe`（可选） | `C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe` |

---

## 1 · 一条命令构建

```powershell
pwsh -File packaging\build-installer.ps1
```

流程：`cargo build --release`（在 `typors/`）→ 校验 payload → 解析版本 → 调 `ISCC.exe` → 输出到 `packaging\dist` → 打印路径/大小/SHA256 → （有证书时）签名。

| 参数 | 说明 |
| --- | --- |
| `-SkipPayloadBuild` | 跳过 `cargo build`，直接用已有的 `typors\target\release\typors.exe` |
| `-Version` | 覆盖版本号；默认读 `typors\Cargo.toml` 的 `[package] version` |
| `-OutDir` | 输出目录，默认 `packaging\dist` |
| `-IsccPath` | 指定 `ISCC.exe`；默认自动发现（用户级 → `Program Files (x86)` → `Program Files` → PATH） |
| `-PayloadPath` | 指定要打包的 exe；默认 `typors\target\release\typors.exe`（指定后不再编译 crate） |

失败即报错退出，不静默继续：payload 缺失、`ISCC.exe` 找不到、`cargo build` 失败、签名工具/证书缺失都会抛出明确错误。

> 注意：脚本内部用 `Invoke-Native` 调用外部工具。原因是 Windows PowerShell 5.1 在 `$ErrorActionPreference='Stop'` 下会把子进程的 **stderr** 当成终止性错误——cargo 的一条普通 warning 就足以让构建失败。该问题已实测遇到并修复。

---

## 2 · 安装界面语言：简体中文（默认）+ 英文（可选）

### 语言文件来源

| 项 | 值 |
| --- | --- |
| 文件 | `packaging\languages\ChineseSimplified.isl` |
| 下载 URL | `https://raw.githubusercontent.com/jrsoftware/issrc/refs/heads/main/Files/Languages/ChineseSimplified.isl`（链接取自官方第三方翻译索引页 `https://jrsoftware.org/files/istrans/`） |
| 获取日期 | **2026-10-25** |
| 文件自述支持版本 | `; *** Inno Setup version 6.5.0+ Chinese Simplified messages ***`（索引页同步标注 `6.5.0+`） |
| 实测编译 | ISCC **6.7.3** `Successful compile`，无 warning；并在运行时读回中文（见下） |
| 维护者 | Zhenghan Yang (Kira) |
| 大小 / 编码 | 21,516 B / **UTF-8 无 BOM** |

### 关于 BOM（实测结论，与常见说法不同）

上游 raw 文件**没有 BOM**（首字节 `3B 20 2A 2A` = `; **`）。常见说法是「ISL 必须 UTF-8 with BOM 否则中文乱码」，本环境实测**并非如此**：无 BOM 的文件被 ISCC 6.7.3 正确解码，中文在运行时完全正常（证据见下一节）。因此这个文件**保持与上游逐字节一致**，没有额外改写成带 BOM，便于日后核对来源。

真正**必须**带 BOM 的是 `packaging\typors.iss`——它含中文自定义消息（`[CustomMessages]` 里的任务文案）。该文件已保存为 UTF-8 with BOM（首字节 `EF BB BF`）。

### 默认中文 + 英文可选 + 不出现语言选择页

```ini
ShowLanguageDialog=no
LanguageDetectionMethod=none

[Languages]
Name: "chinesesimplified"; MessagesFile: "languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"
```

- `[Languages]` **第一项即默认语言** → 中文为默认。
- `ShowLanguageDialog=no` 保证**不会多出一个语言选择页**。
- `LanguageDetectionMethod=none` 关掉自动语言探测，否则在英文 UI 的 Windows 上探测可能把默认语言抢走。
- `/LANG=` 用的是 `[Languages]` 的 `Name`（`chinesesimplified`），**不是** `.isl` 里的 `LanguageName=简体中文`（那只是显示名）。所以 `/LANG=chinesesimplified` / `/LANG=english` 都可用。

### 两条关键文案的实际中文值（已从**编译后的安装器**读回）

用 `[Code]` 的 `SetupMessage(msg…)` + `Log()` 把运行时字符串写进安装日志（`/LOG=`），即「读回编译后的字符串」：

| 位置 | 消息 ID | 实际值 |
| --- | --- | --- |
| **安装模式选择页标题** | `msgPrivilegesRequiredOverrideTitle` | `选择安装程序安装模式` |
| **安装目录页主文案** | `msgSelectDirLabel3` | `安装程序将安装 [name] 到下面的文件夹中。` |

同页其它相关文案：`SelectDirDesc=您想将 [name] 安装在哪里？`、`PrivilegesRequiredOverrideAllUsers=为所有用户安装(&A)`、`PrivilegesRequiredOverrideCurrentUser=仅为我安装(&M)`。

三种语言场景实测（同一个安装器）：

| 调用 | 读回的模式页标题 | 读回的目录页文案 | 读回的自定义任务文案 |
| --- | --- | --- | --- |
| 默认（无 `/LANG`） | `选择安装程序安装模式` | `安装程序将安装 [name] 到下面的文件夹中。` | `将 typors 设为 .md 文件的默认打开程序` |
| `/LANG=chinesesimplified` | 同上（中文） | 同上（中文） | 同上（中文） |
| `/LANG=english` | `Select Setup Install Mode` | `Setup will install [name] into the following folder.` | `Set typors as the default app for .md files` |

补充实测：`/LANG=nonexistent` 时 Inno **静默回退到默认语言**（中文），不报错、退出码仍为 0。所以「语言是否可用」不能靠退出码判断，只能靠上面的字符串读回。

---

## 3 · `.md` 默认打开程序

由 `[Tasks]` 里的 **`mdassoc`** 任务控制，**默认勾选**，用户可取消；取消后注册表**完全不写**（已实测）。`ChangesAssociations=yes` 让 Explorer 立即刷新关联，不必重新登录。

### 写入的内容

根键用 `HKA`：**仅我** → `HKCU\Software\Classes`，**所有用户** → `HKLM\Software\Classes`。

| 键（相对 `Software\Classes`） | 值名 | 值 |
| --- | --- | --- |
| `typors.md` | (默认) | `typors Markdown document` |
| `typors.md\DefaultIcon` | (默认) | `{app}\typors.exe,0` |
| `typors.md\shell\open\command` | (默认) | `"{app}\typors.exe" "%1"` |
| `.md` | (默认) | `typors.md` ← 设为默认打开程序 |
| `.md\OpenWithProgids` | `typors.md` | (空) ← 让它出现在「打开方式」里 |

### 卸载如何还原

- `typors.md` 整个 ProgID 键：`uninsdeletekey` 整键删除——这个键完全属于我们。
- `.md\OpenWithProgids` 下的 `typors.md` **值**：`uninsdeletevalue`——只删**我们这个值名**，同键下其它程序的值（实测本机原有 `Windsurf.md`）不受影响。
- `.md` 的**默认值**：**故意不用** `uninsdeletevalue`。那个 flag 会在卸载时删掉「当前」默认值，哪怕用户早就换成了别的程序，属于误伤。改用 `[Code]` 判断：

```pascal
if RegQueryStringValue(ClassesRootKey, 'Software\Classes\.md', '', Current)
   and (CompareText(Current, MdProgId) = 0) then
  RegDeleteValue(ClassesRootKey, 'Software\Classes\.md', '');
```

即**只在 `.md` 仍指向 `typors.md` 时才删**。`.md` 键本身不删（可能承载别人的数据）。`ClassesRootKey` 用 `IsAdminInstallMode` 在 `HKLM`/`HKCU` 间选择，与 `HKA` 语义一致。

> 已知语义边界：如果安装前 `.md` 已有一个**默认处理程序**，安装会覆盖它，卸载**不会恢复**原来那个（Inno 没有内置的「保存并恢复」机制）。卸载只是把默认值清空，让系统回到「请你选择打开方式」。如需保留旧值需额外实现备份/恢复逻辑，当前版本未做。

### ⚠️ Windows 10/11 的 `UserChoice` 限制（重要，非脚本缺陷）

自 Windows 8 起，某个扩展名的「默认程序」由 `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.md\UserChoice` 说了算，**该键受系统 ACL 保护，安装器无法合法写入**。后果：

- 我们写 `Software\Classes\.md` 的默认值，是「**登记为默认**」；若用户此前已为该扩展名做过选择（`UserChoice` 存在），系统**仍按 `UserChoice` 打开**。
- 表现可能是：双击 `.md` 仍然用旧程序打开，或系统弹出「你要如何打开这个文件？」让用户确认一次。
- **这是 Windows 的保护机制，不是安装脚本有 bug。**

本机实测就是这个状态：安装后 `Software\Classes\.md` 已正确变为 `typors.md`，而 `UserChoice` 仍是用户原来的 `Typora.md`（安装与卸载全程未被改动）。

**用户如何手动生效/解除：**

- 让 typors 成为 `.md` 的默认：右键任意 `.md` → 打开方式 → 选择其他应用 → typors → 勾选「始终使用此应用打开 .md 文件」；或「设置 → 应用 → 默认应用 → 按文件类型选择 .md → typors」。
- 解除关联（不卸载程序也可以）：同上把 `.md` 改回想要的程序即可；把默认值指向别处后，卸载 typors 不会改动它。
- 若确实想让系统忘记该扩展名的旧选择，需删除 `UserChoice` 子键（受 ACL 保护，需高级权限/专门工具），本安装器**不会**碰它。

---

## 4 · 两种安装模式落到哪里

| | 仅我（默认） | 所有用户 |
| --- | --- | --- |
| 权限要求 | 无（`PrivilegesRequired=lowest`） | **管理员 / UAC 提权** |
| 默认安装目录 | `{autopf}` → `%LocalAppData%\Programs\typors` | `{autopf}` → `C:\Program Files\typors` |
| 程序文件 | `<安装目录>\typors.exe` | 同左 |
| 卸载数据 | `<安装目录>\unins000.exe` / `unins000.dat` | 同左 |
| 卸载注册表项 | `HKCU\…\Uninstall\{f3c62e7d-9603-40b2-af12-ee14933a91a6}_is1` | `HKLM\…\Uninstall\{…}_is1` |
| 文件关联（`HKA`） | `HKCU\Software\Classes` | `HKLM\Software\Classes` |
| 开始菜单 | `{autoprograms}\typors\typors.lnk` → `%AppData%\Microsoft\Windows\Start Menu\Programs\typors\typors.lnk` | 同结构，`%ProgramData%\…` |
| 桌面快捷方式 | `{autodesktop}\typors.lnk`，**仅在勾选 `desktopicon` 任务时创建**（默认不勾） | 同左 |

`{autopf}` 是 Inno 的标准常量：管理员模式解析为 `Program Files`，非管理员模式解析为 `%LocalAppData%\Programs`。**同一份脚本因此不需要任何分支**就能让两种模式各自落到正确位置。

[Icons] 用的是 `{group}\typors` 而不是 `{autoprograms}\typors`，这样 `/NOICONS` 和「不创建开始菜单文件夹」才真正生效。

### 卸载入口

- 「设置 → 应用 → 已安装的应用」中的 **typors**（来自上面的卸载注册表项）
- 或直接运行 `<安装目录>\unins000.exe`
- 静默卸载：`unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART`

卸载会移除程序文件、开始菜单快捷方式（含 `typors` 文件夹）、桌面快捷方式（若曾创建）、卸载注册表项、`typors.md` ProgID、`.md\OpenWithProgids` 下我们的值，并在 `.md` 仍指向我们时清空其默认值，最后删除安装目录本身。

### 选择模式的入口

- **图形向导**：首屏即 `选择安装程序安装模式`（中文）/ `Select Setup Install Mode`（英文）页，选项为 `为所有用户安装(A)` / `仅为我安装(M)`。由 `PrivilegesRequiredOverridesAllowed=dialog` 提供。
- **命令行**：`/ALLUSERS` 与 `/CURRENTUSER`（由 `PrivilegesRequiredOverridesAllowed=commandline` 提供）。
- **安装目录**：向导的「选择目标位置」页始终显示（`DisableDirPage=no`）；命令行用 `/DIR="<路径>"`。

---

## 5 · AppId：**升级必须沿用**

```
AppId={{f3c62e7d-9603-40b2-af12-ee14933a91a6}
```

`AppId` 是 Windows 视角下「这是哪个产品」的唯一身份，用来生成卸载注册表项、让新版本安装器识别并**替换**旧版本。因此：

- **升级时绝对不要改这个 GUID**。改了之后新包会被当作另一个产品，结果是**并存安装**（两个 typors、两个卸载项），而不是升级。
- 只有在刻意更换产品身份（例如分支出一个不兼容的独立产品）时才换新 GUID。

`AppVersion` 每次发版更新（默认取 `Cargo.toml`）。`VersionInfoVersion` 只接受数字点分形式，构建脚本会自动把 `0.2.0-rc1` 这类值的数字前缀单独传给 `VersionQuad`。

---

## 6 · 签名（默认关闭）

**默认构建不签名**，且在没有证书时**不会报错**：`[Setup]` 里的 `SignTool` 指令被 `#ifdef SIGNTOOL` 包住，未定义时该指令根本不存在，因此不需要预先注册任何签名工具。

开启方式 A —— 让构建脚本签最终的安装包（推荐，已实现）：

```powershell
$env:TYPORS_SIGN_CERT     = 'C:\path\to\cert.pfx'   # 必填；设置即启用
$env:TYPORS_SIGN_PASSWORD = '********'              # 可选
$env:TYPORS_TIMESTAMP_URL = 'http://timestamp.digicert.com'  # 建议设置
pwsh -File packaging\build-installer.ps1
```

`TYPORS_SIGN_CERT` 设置后脚本会：找不到 `signtool.exe` 就报错；证书文件不存在就报错（不静默产出未签名包）；未设 `TYPORS_TIMESTAMP_URL` 时给出 warning（签名会随证书过期）。

开启方式 B —— 让 Inno 在编译期签名（含内嵌卸载器）：

```powershell
ISCC /DSIGNTOOL=typors "/Stypors=signtool sign /f cert.pfx /p *** /fd sha256 $f" packaging\typors.iss
```

`/Stypors=` 的名字必须与 `SignTool=` 的值一致。这条路径本次**未实测**（无证书）。

### SmartScreen

未签名的安装包在别的机器上首次运行会触发 **"Windows 已保护你的电脑"**（SmartScreen）。这是预期行为，不是安装器缺陷。要消除它只有两条路：购买代码签名证书并做 EV/口碑签名，或者在用户机器上手动「更多信息 → 仍要运行」。

---

## 7 · 验证状态（如实）

环境固定事实：身份 `chenzhi\chenzhi`，`IsInRole(Administrator) == False`，无交互式桌面。

| 项 | 状态 | 依据 |
| --- | --- | --- |
| 自选安装目录 | **已验证** | `/DIR="…\typors p3 assoc"` 静默安装后文件确实落在该目录（含空格路径） |
| 仅我安装（`/CURRENTUSER`） | **已验证** | 无 `/DIR` 时落在 `%LocalAppData%\Programs\typors`；`Program Files\typors` 未生成 |
| 中文为默认语言 | **已验证** | 无 `/LANG` 时从编译后的安装器读回中文文案（§2 表） |
| `/LANG=english` 可用 | **已验证** | 同上表，读回英文文案；静默安装 exit 0 |
| `/LANG=chinesesimplified` 可用 | **已验证** | 同上表；静默安装 exit 0 |
| 无语言选择页 | **仅指令级** | `ShowLanguageDialog=no` + `LanguageDetectionMethod=none`；无交互桌面无法目视向导页数 |
| `.md` 关联写入正确 | **已验证** | 读回 `.md` 默认值、`typors.md\shell\open\command`、`DefaultIcon`、`OpenWithProgids`（§3） |
| 取消 `mdassoc` 不写注册表 | **已验证** | `/MERGETASKS="!mdassoc"` 安装后 `typors.md` 不存在、`.md` 默认值仍为空 |
| 卸载清理关联且不误伤 | **已验证** | 卸载后 `typors.md` 键消失、`.md` 默认值清空、`OpenWithProgids` 只剩原有 `Windsurf.md` |
| 卸载清理程序/快捷方式/卸载项 | **已验证** | 目录、`typors` 文件夹 + `typors.lnk`、HKCU 卸载项全部消失 |
| `UserChoice` 不被改动 | **已验证** | 安装/卸载前后均仍为本机原有的 `Typora.md` |
| 桌面快捷方式 | **部分验证** | 只验证了「默认不勾 → 不创建」；勾选后创建未跑 |
| 所有用户安装（`/ALLUSERS`） | **未验证** | 卡在 UAC，见 §8 |
| 向导页面的**视觉**呈现 | **未验证** | 无交互桌面；文案正确性靠运行时字符串读回证明，非截图 |
| 签名 | **未验证** | 无证书，默认路径为「不签名」 |

---

## 8 · 已知限制

1. **「所有用户」分支未真实跑通。** 本环境非管理员，`/ALLUSERS` 安装时 Inno 会请求提权并显示 UAC 同意框。实测把安装器挂在那里等 120 秒仍未返回，期间观察到 `consent.exe`（Windows UAC 同意代理）与 `typors-0.1.0-spike-setup.tmp`（Inno 提权后的子进程）同时存在，说明**提权请求确实发出去了**；因为没有可交互的同意框，进程既没完成也没报错，最终被强杀。后果：`Program Files\typors` 与 HKLM 卸载项都没有产生，机器上无残留。**所以「所有用户」这条路径的正确性属于推断，不属于已验证**——需要在一台能点 UAC 的桌面机上补测（同时可顺带验证 `HKLM\Software\Classes` 的关联落点）。
2. **`UserChoice` 限制**：见 §3，Windows 10/11 上仅写注册表无法强行改写已有默认程序。
3. **卸载不恢复原有 `.md` 默认程序**：见 §3 的语义边界说明。
4. **未签名**：见 §6，会触发 SmartScreen。
5. **向导为中文，但 `WizardStyle=modern` 的图片未定制**：沿用 Inno 默认外观。

---

## 9 · 手动验证最小步骤（用户自己跑）

1. 打开 `packaging\dist\typors-0.1.0-setup.exe`（双击）。
2. **第一屏**应是中文 `选择安装程序安装模式`，两个选项：`为所有用户安装(A)` 与 `仅为我安装(M)`。→ 验证「可选安装模式」+ 中文界面。
3. 点「下一步」，在 `选择目标位置` 页**目录路径可编辑**，可改成任意目录。→ 验证「可选安装目录」。
4. `选择附加任务` 页应看到 **`文件关联`** 组下的 **`将 typors 设为 .md 文件的默认打开程序`（默认已勾选）**，可取消。另外 `创建桌面快捷方式` 默认不勾。
5. 装完检查：开始菜单 `typors` 文件夹下有 `typors` 快捷方式；「设置 → 应用」里有 typors；右键某个 `.md` 看「打开方式」里是否出现 typors。
6. 若双击 `.md` 仍用旧程序打开，按 §3 的「用户如何手动生效」做一次确认——**这是 `UserChoice` 保护机制，属预期**。
7. 卸载：设置 → 应用 → typors → 卸载；或 `<安装目录>\unins000.exe`。卸载后确认 `HKCU\Software\Classes\typors.md` 已消失、`.md` 默认值被清空。

自动化等价命令：

```powershell
# 仅我 + 指定目录（中文界面，默认任务含 mdassoc）
.\packaging\dist\typors-0.1.0-setup.exe /CURRENTUSER /DIR="D:\tools\typors" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-
# 英文界面
.\packaging\dist\typors-0.1.0-setup.exe /CURRENTUSER /LANG=english /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-
# 不要文件关联
.\packaging\dist\typors-0.1.0-setup.exe /CURRENTUSER /MERGETASKS="!mdassoc" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-
# 所有用户（需管理员）
.\packaging\dist\typors-0.1.0-setup.exe /ALLUSERS /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-
```

---

## 10 · 关键脚本片段

```ini
DefaultDirName={autopf}\typors
DisableDirPage=no
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
AppId={{f3c62e7d-9603-40b2-af12-ee14933a91a6}
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesAssociations=yes
ShowLanguageDialog=no
LanguageDetectionMethod=none
#ifdef SIGNTOOL
SignTool={#SIGNTOOL}
#endif

[Languages]
Name: "chinesesimplified"; MessagesFile: "languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "mdassoc"; Description: "{cm:MdAssocTask}"; GroupDescription: "{cm:MdAssocGroup}"

[Registry]
Root: HKA; Subkey: "Software\Classes\typors.md"; ValueType: string; ValueName: ""; ValueData: "typors Markdown document"; Tasks: mdassoc; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\typors.md\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExeName},0"; Tasks: mdassoc
Root: HKA; Subkey: "Software\Classes\typors.md\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""; Tasks: mdassoc
Root: HKA; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "typors.md"; ValueData: ""; Tasks: mdassoc; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.md"; ValueType: string; ValueName: ""; ValueData: "typors.md"; Tasks: mdassoc
```
