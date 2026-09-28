import SwiftUI

// Plain records standing in for the UniFFI records of NiumaTermCore (design doc §4.2).

enum AgentProfile: String, CaseIterable, Identifiable, Hashable {
    case claude = "Claude Code"
    case codex = "Codex"
    case deepseek = "DeepSeek"

    var id: String { rawValue }
    var shortName: String {
        switch self {
        case .claude: "Claude"
        case .codex: "Codex"
        case .deepseek: "DeepSeek"
        }
    }
    var glyph: String {
        switch self {
        case .claude: "✱"
        case .codex: "◎"
        case .deepseek: "◆"
        }
    }
    var models: [String] {
        switch self {
        case .claude: ["opus[1m]", "sonnet", "haiku"]
        case .codex: ["gpt-5-codex", "gpt-5"]
        case .deepseek: ["deepseek-v4", "deepseek-r2"]
        }
    }
}

enum SessionKind: Hashable {
    case terminal
    case agent(AgentProfile)
}

/// `sessions.list` activity field (design doc §13 item 5).
enum Activity: Hashable {
    case idle(since: String?)
    case working(elapsed: String)
    case needsApproval
    case asksQuestion
}

struct Session: Identifiable {
    let id: String
    var title: String
    var kind: SessionKind
    var activity: Activity
    var controlledOnDesktop = false
    var cwd: String

    var profile: AgentProfile? {
        if case .agent(let p) = kind { return p }
        return nil
    }

    var glyph: String { profile?.glyph ?? ">_" }

    var statusText: String {
        guard let profile else { return "Terminal · idle" }
        let prefix = profile == .claude ? "" : "\(profile.shortName) · "
        switch activity {
        case .idle(let since): return prefix + (since.map { "Idle · \($0)" } ?? "Idle")
        case .working(let elapsed): return prefix + "● Working · \(elapsed)"
        case .needsApproval: return prefix + "◌ Needs approval"
        case .asksQuestion: return prefix + "? Asks a question"
        }
    }

    var statusColor: Color {
        switch activity {
        case .working: Theme.accent
        case .needsApproval, .asksQuestion: Theme.attention
        case .idle: Theme.secondary
        }
    }
}

struct Workspace: Identifiable {
    let id: String
    var name: String
    var path: String
    var sessions: [Session]

    var shortPath: String {
        let parts = path.split(separator: "\\")
        guard parts.count > 2, let last = parts.last else { return path }
        return "…\\" + last
    }
}

enum HostStatus {
    case online
    case connecting
    case offline(lastSeen: String)
}

struct Host: Identifiable {
    let id: String
    var name: String
    var isLaptop: Bool
    var status: HostStatus
    var workspaces: [Workspace]

    var isOnline: Bool {
        if case .online = status { return true }
        return false
    }
    var icon: String { isLaptop ? "laptopcomputer" : "desktopcomputer" }
    var statusText: String {
        switch status {
        case .online: "Online · via relay"
        case .connecting: "Connecting…"
        case .offline(let seen): "Offline · last seen \(seen)"
        }
    }
    var statusColor: Color {
        switch status {
        case .online: Theme.online
        case .connecting: Theme.attention
        case .offline: Theme.offline
        }
    }
}

struct ToolCall: Identifiable {
    let id = UUID()
    let kind: String
    let detail: String
    init(_ kind: String, _ detail: String) {
        self.kind = kind
        self.detail = detail
    }
}

struct TranscriptEntry: Identifiable {
    enum Kind {
        case user(String)
        case agent(String)
        case thinking(String)
        case tools([ToolCall])
        case notice(String, Color)
    }
    let id: Int
    let kind: Kind
}

struct QueuedPrompt: Identifiable {
    let id = UUID()
    let text: String
}

/// The `pending` slot.
struct ApprovalRequest: Identifiable {
    let id = UUID()
    var title: String
    var summary: String
    var command: String
    var cwd: String
    var ruleLabel: String
}
