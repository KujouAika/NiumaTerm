import SwiftUI
import NiumaTermCore

/// An agent as a host names it in `host.info` and on its sessions.
struct AgentProfile: Hashable {
    let harness: String

    var displayName: String {
        switch harness {
        case "claude": "Claude Code"
        case "codex": "Codex"
        case "deepseek": "DeepSeek"
        default: harness.capitalized
        }
    }

    var shortName: String {
        switch harness {
        case "claude": "Claude"
        case "codex": "Codex"
        case "deepseek": "DeepSeek"
        default: harness.capitalized
        }
    }

    var glyph: String {
        switch harness {
        case "claude": "✱"
        case "codex": "◎"
        case "deepseek": "◆"
        default: "◇"
        }
    }
}

struct Session: Identifiable, Hashable {
    let id: String
    let hostID: String
    var title: String
    var kind: SessionType
    var profile: AgentProfile?

    init(record: SessionRecord, hostID: String) {
        id = record.id
        self.hostID = hostID
        title = record.title
        kind = record.kind
        profile = record.harness.map(AgentProfile.init(harness:))
    }

    var glyph: String {
        switch kind {
        case .terminal: ">_"
        case .agent: profile?.glyph ?? "✱"
        case .other: "?"
        }
    }

    var subtitle: String {
        switch kind {
        case .terminal: "Terminal"
        case .agent: profile?.displayName ?? "Agent"
        case .other: "Session"
        }
    }
}

/// A route on the navigation stack: one session on one host. It carries
/// the kind because a session just opened here may reach the host's list
/// only after its screen is already showing.
struct SessionRoute: Hashable {
    let hostID: String
    let sessionID: String
    let kind: SessionType
}

struct Host: Identifiable {
    let id: String
    var name: String
    var status: HostStatus
    var sessions: [Session] = []

    var isOnline: Bool { status == .connected }
    var icon: String { "desktopcomputer" }

    var statusText: String {
        switch status {
        case .connected: "Online"
        case .connecting: "Connecting…"
        case .reconnecting: "Reconnecting…"
        case .idle: "Not connected"
        case .refused: "No longer trusts this phone · pair again"
        case .unreachable: "Connection failed"
        }
    }

    var statusColor: Color {
        switch status {
        case .connected: Theme.online
        case .connecting, .reconnecting: Theme.attention
        case .idle: Theme.offline
        case .refused, .unreachable: Theme.attention
        }
    }

    /// Terminals first, then agents, as the design lists them.
    var orderedSessions: [Session] {
        sessions.filter { $0.kind == .terminal } + sessions.filter { $0.kind != .terminal }
    }
}

extension Error {
    /// The core words its errors for people already.
    var displayText: String {
        if let core = self as? CoreError, case .Failed(let message) = core {
            return message
        }
        return localizedDescription
    }
}
