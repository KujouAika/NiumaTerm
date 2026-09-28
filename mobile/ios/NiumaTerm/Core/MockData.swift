import SwiftUI

/// Canned terminal output for the terminal prototype, until the terminal
/// milestone draws the core's grid.
enum MockData {
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
