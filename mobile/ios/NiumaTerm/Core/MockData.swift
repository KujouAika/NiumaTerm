import SwiftUI

/// Demo data mirroring the desktop screenshots. Replace with NiumaTermCore calls.
enum MockData {
    static let hosts: [Host] = [
        Host(id: "studio", name: "Studio PC", isLaptop: false, status: .online, workspaces: [
            Workspace(id: "w-niuma", name: "NiumaTerm", path: "C:\\Workspace\\NiumaTerm", sessions: [
                Session(id: "s-ps1", title: "PowerShell", kind: .terminal, activity: .idle(since: nil), cwd: "C:\\Workspace\\NiumaTerm"),
                Session(id: "s-tab", title: "适配 Agent Tab 换行快捷键", kind: .agent(.claude), activity: .idle(since: "12m"), cwd: "C:\\Workspace\\NiumaTerm"),
                Session(id: "s-remote-impl", title: "远程会话实现", kind: .agent(.claude), activity: .working(elapsed: "3m 40s"), controlledOnDesktop: true, cwd: "C:\\Workspace\\NiumaTerm"),
                Session(id: "s-remote-design", title: "远程会话设计开发", kind: .agent(.claude), activity: .needsApproval, cwd: "C:\\Workspace\\NiumaTerm"),
            ]),
            Workspace(id: "w-dumb", name: "Dumb Browser", path: "C:\\Workspace\\dumb\\src", sessions: [
                Session(id: "s-ps2", title: "PowerShell", kind: .terminal, activity: .idle(since: nil), cwd: "C:\\Workspace\\dumb\\src"),
                Session(id: "s-mica", title: "查找 Mica 下 Tab 灰色背景渲染源", kind: .agent(.codex), activity: .idle(since: "1h"), cwd: "C:\\Workspace\\dumb\\src"),
            ]),
            Workspace(id: "w-sync", name: "NiumaTerm-sync", path: "C:\\Workspace\\NiumaTerm-sync", sessions: [
                Session(id: "s-ps3", title: "PowerShell", kind: .terminal, activity: .idle(since: nil), cwd: "C:\\Workspace\\NiumaTerm-sync"),
                Session(id: "s-sync-cc", title: "Claude Code", kind: .agent(.claude), activity: .idle(since: "2d"), cwd: "C:\\Workspace\\NiumaTerm-sync"),
                Session(id: "s-sync-cx", title: "Codex", kind: .agent(.codex), activity: .idle(since: "2d"), cwd: "C:\\Workspace\\NiumaTerm-sync"),
            ]),
            Workspace(id: "w-park", name: "Park", path: "D:\\Park", sessions: [
                Session(id: "s-park", title: "Codex", kind: .agent(.codex), activity: .asksQuestion, cwd: "D:\\Park"),
            ]),
        ]),
        Host(id: "air", name: "MacBook Air", isLaptop: true, status: .offline(lastSeen: "2h ago"), workspaces: []),
    ]

    struct AgentSeed {
        var entries: [TranscriptEntry]
        var working: Bool
        var elapsed: TimeInterval
        var approval: ApprovalRequest?
    }

    static func agentSeed(for session: Session) -> AgentSeed {
        func numbered(_ kinds: [TranscriptEntry.Kind]) -> [TranscriptEntry] {
            kinds.enumerated().map { TranscriptEntry(id: $0.offset, kind: $0.element) }
        }
        switch session.id {
        case "s-remote-impl":
            return AgentSeed(entries: numbered([
                .user("完美，提交改动吧"),
                .tools([ToolCall("Bash", "git status --short"), ToolCall("Bash", "git diff --stat"), ToolCall("Bash", "git log -3 --oneline")]),
                .agent("I'll split this into three commits: the sidebar, the live sheet, and the doc."),
                .tools([ToolCall("Bash", "git add crates/app/src/sidebar"), ToolCall("Bash", "git commit -m \"sidebar\""), ToolCall("Read", ".githooks/commit-msg"), ToolCall("Bash", "cargo clippy")]),
                .agent("The hook's clippy flagged my overlay edit (a collapsible match), so I'll fix that first."),
                .user("再给设置里忘记配对的按钮增加二次确认，我刚才不小心点了导致配对直接消失了"),
                .thinking("The commit-msg hook rejects commits without the model ID trailer. Redo the split first, then add the confirmation dialog."),
                .agent("The commit-msg hook wants the model ID in the trailer, so I'm fixing that and redoing the split. After that I'll add the Forget confirmation."),
                .tools([ToolCall("Bash", "git commit --amend"), ToolCall("Bash", "git log -3")]),
                .agent("Committed. Now the Forget confirmation."),
                .tools([ToolCall("Read", "settings/remote.rs"), ToolCall("Grep", "ConfirmDialog"), ToolCall("Edit", "settings/remote.rs"), ToolCall("Bash", "cargo check -p nmt_app")]),
                .agent("Wiring a confirmation dialog onto the Forget button, following the existing settings dialog pattern."),
            ]), working: true, elapsed: 220, approval: nil)
        case "s-remote-design":
            return AgentSeed(entries: numbered([
                .user("把设计文档提交一下"),
                .tools([ToolCall("Bash", "git add docs/research/ios-app-design.md"), ToolCall("Bash", "git diff --cached --stat")]),
                .agent("Changes are staged. Committing with the model ID in the trailer."),
                .notice("◌ Waiting for approval", Theme.attention),
            ]), working: false, elapsed: 0, approval: ApprovalRequest(
                title: "\(session.profile?.shortName ?? "Agent") needs approval",
                summary: "Run a command in NiumaTerm",
                command: "git commit -m \"docs: iOS app design\"",
                cwd: session.cwd,
                ruleLabel: "Allow git commit for this session"))
        case "s-tab":
            return AgentSeed(entries: numbered([
                .user("Agent tab 里 Shift+Enter 换行，Enter 发送"),
                .tools([ToolCall("Grep", "KeyBinding"), ToolCall("Edit", "agent/composer.rs")]),
                .agent("Done. Shift+Enter now inserts a newline in the agent composer and Enter sends. The keymap entry is in `keymaps/default.toml`."),
            ]), working: false, elapsed: 0, approval: nil)
        default:
            return AgentSeed(entries: [], working: false, elapsed: 0, approval: nil)
        }
    }

    // MARK: Terminal

    private enum Style { case plain, dir, link }

    private static func line(_ prefix: String, _ name: String = "", _ suffix: String = "", _ style: Style = .plain) -> AttributedString {
        var out = AttributedString(prefix)
        var n = AttributedString(name)
        switch style {
        case .dir:
            n.backgroundColor = Theme.directory
            n.foregroundColor = Color.white
        case .link:
            n.foregroundColor = Theme.directory
        case .plain:
            break
        }
        out += n
        out += AttributedString(suffix)
        return out
    }

    private static func pad(_ s: String, _ width: Int) -> String {
        String(repeating: " ", count: max(0, width - s.count)) + s
    }

    static func listing(cwd: String) -> [AttributedString] {
        let dirs: [(String, String, String)] = [
            ("9/15/2026", "10:13 PM", ".cargo"), ("9/26/2026", "10:32 PM", ".claude"),
            ("8/14/2026", "10:17 PM", ".codex"), ("9/24/2026", " 9:38 PM", ".githooks"),
            ("9/27/2026", " 9:07 PM", ".scratch"), ("9/26/2026", "11:21 PM", ".worktrees"),
            ("9/25/2026", "10:10 AM", "assets"), ("9/27/2026", "11:48 AM", "crates"),
            ("9/24/2026", " 9:38 PM", "docs"), ("9/27/2026", " 2:57 PM", "relay"),
            ("9/24/2026", "10:36 PM", "scripts"), ("9/18/2026", " 9:33 PM", "target"),
        ]
        let files: [(String, String, String, String, String)] = [
            ("9/14/2026", "10:13 PM", "8118", "AGENTS.md", ""),
            ("9/27/2026", " 1:30 PM", "237772", "Cargo.lock", ""),
            ("9/27/2026", " 6:15 PM", "20880", "Cargo.toml", ""),
            ("8/9/2026", " 1:51 PM", "0", "CLAUDE.md", " -> AGENTS.md"),
            ("9/18/2026", " 9:15 PM", "3363", "README.md", ""),
            ("9/27/2026", " 2:41 PM", "310", "tsconfig.json", ""),
        ]
        var out: [AttributedString] = [
            line(""), line("    Directory: \(cwd)"), line(""),
            line("Mode   LastWriteTime          Length Name"),
            line("----   -------------          ------ ----"),
        ]
        for (d, t, n) in dirs {
            out.append(line("d----  \(pad(d, 9)) \(t)        ", n, "", .dir))
        }
        for (d, t, size, n, suffix) in files {
            out.append(line("-a---  \(pad(d, 9)) \(t) \(pad(size, 6)) ", n, suffix, suffix.isEmpty ? .plain : .link))
        }
        out.append(line(""))
        return out
    }

    static func output(for command: String, cwd: String) -> [AttributedString] {
        switch command.trimmingCharacters(in: .whitespaces) {
        case "":
            return []
        case "ls", "dir", "gci", "Get-ChildItem":
            return listing(cwd: cwd)
        case "git status", "git st":
            return [
                line("On branch feature/remote-session"),
                line("Changes not staged for commit:"),
                line("        ", "modified:   crates/app/src/settings/remote.rs", "", .link),
                line(""),
            ]
        default:
            return [line("(demo) sent to host: \(command)")]
        }
    }
}
