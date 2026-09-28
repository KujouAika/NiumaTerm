# NiumaTerm iOS — Demo 工程

纯 SwiftUI 的界面 Demo，数据全部是 Mock，结构按 `ios-app-design.md` §5 组织，可以直接放进仓库的 `mobile/ios/`。

## 打开

1. Mac 上需要 Xcode 26（iOS 26 SDK）。
2. 双击 `NiumaTerm.xcodeproj`。
3. Signing & Capabilities 里选择你的 Team；如有需要，改掉 Bundle ID `com.example.niumaterm`。
4. 选一台 iPhone 模拟器，按 ⌘R 运行。

工程使用 Xcode 16 起支持的文件夹同步，`NiumaTerm/` 下新建的文件会自动加入 target。
`project.yml` 是 XcodeGen 配置，以后加 Notification Service Extension 和 NiumaTermCore 包时再用 `xcodegen` 重新生成工程。

## 界面与文件

| 界面 | 文件 |
| --- | --- |
| 电脑与会话列表（1a） | `Hosts/HostListView.swift` |
| 新建会话（1b） | `Hosts/NewSessionSheet.swift` |
| Agent 会话、对话记录、输入框（1c） | `Agent/AgentSessionView.swift`、`Agent/ComposerView.swift` |
| 审批、桌面收回控制（1d、1g） | `Agent/AgentSheets.swift` |
| 终端 + 键盘附件栏（1e） | `Terminal/TerminalSessionView.swift` |
| 扫码配对 + 手动输入（1f） | `Hosts/PairingView.swift`（真机上使用 VisionKit 的 DataScanner） |
| 设置 | `Settings/SettingsView.swift` |
| 颜色、字体 | `Theme/Theme.swift` |
| Liquid Glass 与 iOS 18 回退 | `Compat/Compat.swift` |

## Demo 里能试的

- Agent 会话右上角 `···` → Demo：模拟审批请求、模拟桌面收回控制。
- Agent 工作中输入消息会进入排队列表；没有输入时按按钮会中断。
- 终端：点屏幕弹出键盘，输入 `ls`、`git status`、`clear`；Ctrl 为粘滞键（Ctrl 之后按 C 会显示 ^C）；双指捏合调整字号。
- 模拟器上配对页有一个 “Simulate scan” 按钮。

## 接入真实 Core 时要替换的部分

- `AppModel` 替换为 `MobileCore`：`hosts()`、`observe`、`open_terminal`、`open_agent`、`pair`、`forget`。
- `AgentSessionModel` 替换为 `AgentHandle` + `AgentObserver`：各属性分别对应 `status`、`settings`、`queue`、`pending` 这几个 slot。
- `TerminalSessionModel` + `TerminalSessionView` 的文本网格替换为 UIKit 的 `TerminalSurface`（Core Text 按行绘制），输入换成实现了 `UITextInput` 的视图。
- `Core/MockData.swift` 可以整个删除。

## 字体

把 JetBrains Mono Nerd Font Mono 的 ttf 文件放到 `NiumaTerm/Resources/Fonts/`，启动时会自动注册，不需要改 Info.plist。
没有放字体时使用 SF Mono。
