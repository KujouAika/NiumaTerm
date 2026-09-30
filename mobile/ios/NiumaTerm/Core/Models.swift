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
    var workspace: SessionWorkspace?

    init(record: SessionRecord, hostID: String) {
        id = record.id
        self.hostID = hostID
        title = record.title
        kind = record.kind
        profile = record.harness.map(AgentProfile.init(harness:))
        workspace = record.workspace.map {
            SessionWorkspace(id: $0.id, name: $0.name, position: Int($0.position))
        }
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
        case .terminal: tr("Terminal")
        case .agent: profile?.displayName ?? tr("Agent")
        case .other: tr("Session")
        }
    }
}

/// The host workspace whose tab shows a session.
struct SessionWorkspace: Hashable {
    let id: String
    let name: String
    let position: Int
}

/// A host's sessions from one of its workspaces. `workspace` is nil for
/// sessions no workspace claims, and for every session of a host that does
/// not report workspaces.
struct SessionGroup: Identifiable {
    let workspace: SessionWorkspace?
    let sessions: [Session]

    var id: String { workspace?.id ?? "" }
}

/// A route on the navigation stack: one session on one host. It carries
/// the kind because a session just opened here may reach the host's list
/// only after its screen is already showing.
struct SessionRoute: Hashable {
    let hostID: String
    let sessionID: String
    let kind: SessionType
}

/// Which network links to computers may use. Stored by raw value in user
/// defaults, so the cases' names must not change.
enum NetworkPreference: String, CaseIterable, Identifiable {
    case auto
    case relay
    case lan

    static let storageKey = "networkMode"

    var id: String { rawValue }

    var label: String {
        switch self {
        case .auto: tr("Automatic")
        case .relay: tr("Always Relay")
        case .lan: tr("Always LAN")
        }
    }

    var mode: NetworkMode {
        switch self {
        case .auto: .auto
        case .relay: .relay
        case .lan: .lan
        }
    }

    static var stored: NetworkPreference {
        UserDefaults.standard.string(forKey: storageKey).flatMap(NetworkPreference.init(rawValue:)) ?? .auto
    }
}

struct Host: Identifiable {
    let id: String
    var name: String
    var status: HostStatus
    /// How the host is reached, while it is connected.
    var link: HostLink?
    var sessions: [Session] = []

    init(record: HostRecord) {
        id = record.id
        name = record.name
        status = record.status
        link = record.link
    }

    var isOnline: Bool { status == .connected }
    var icon: String { "desktopcomputer" }

    /// The network the link runs over, as session title bars show it.
    var linkText: String? {
        switch link {
        case .lan: tr("LAN")
        case .relay: tr("Relay")
        case nil: nil
        }
    }

    var statusText: String {
        switch status {
        case .connected: tr("Online")
        case .connecting: tr("Connecting…")
        case .reconnecting: tr("Reconnecting…")
        case .idle: tr("Not connected")
        case .refused: tr("No longer trusts this phone · pair again")
        case .unreachable: tr("Connection failed")
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

    /// Sessions grouped by workspace in the host's workspace order, with
    /// unclaimed sessions last; within a group terminals come first, then
    /// agents, as the design lists them.
    var sessionGroups: [SessionGroup] {
        let grouped = Dictionary(grouping: sessions) { $0.workspace?.id }
        return grouped.values
            .map { sessions in
                SessionGroup(workspace: sessions.first?.workspace,
                             sessions: sessions.filter { $0.kind == .terminal } + sessions.filter { $0.kind != .terminal })
            }
            .sorted { a, b in
                switch (a.workspace, b.workspace) {
                case let (a?, b?): (a.position, a.id) < (b.position, b.id)
                case (_?, nil): true
                case (nil, _): false
                }
            }
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
