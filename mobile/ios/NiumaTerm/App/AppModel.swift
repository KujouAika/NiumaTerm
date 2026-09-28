import SwiftUI
import Observation

/// App-wide state. In the real app this wraps `MobileCore` (design doc §4.2)
/// and is fed by `CoreObserver` callbacks.
@MainActor
@Observable
final class AppModel {
    var hosts: [Host] = MockData.hosts
    var path = NavigationPath()

    @ObservationIgnored private var agentModels: [String: AgentSessionModel] = [:]
    @ObservationIgnored private var terminalModels: [String: TerminalSessionModel] = [:]

    func lookup(_ sessionID: String) -> (host: Host, workspace: Workspace, session: Session)? {
        for host in hosts {
            for ws in host.workspaces {
                if let s = ws.sessions.first(where: { $0.id == sessionID }) {
                    return (host, ws, s)
                }
            }
        }
        return nil
    }

    /// `attach_agent` — attaching takes control from the desktop (§8.4).
    func agentModel(for session: Session) -> AgentSessionModel {
        if let m = agentModels[session.id] { return m }
        let m = AgentSessionModel(session: session)
        agentModels[session.id] = m
        return m
    }

    /// `attach_terminal`.
    func terminalModel(for session: Session) -> TerminalSessionModel {
        if let m = terminalModels[session.id] { return m }
        let m = TerminalSessionModel(session: session)
        terminalModels[session.id] = m
        return m
    }

    /// `open_terminal` / `open_agent`.
    func createSession(hostID: String, workspaceID: String, kind: SessionKind) -> String? {
        guard let h = hosts.firstIndex(where: { $0.id == hostID }),
              let w = hosts[h].workspaces.firstIndex(where: { $0.id == workspaceID }) else { return nil }
        let ws = hosts[h].workspaces[w]
        let title: String
        switch kind {
        case .terminal: title = "PowerShell"
        case .agent(let p): title = p.rawValue
        }
        let session = Session(id: UUID().uuidString, title: title, kind: kind, activity: .idle(since: nil), cwd: ws.path)
        hosts[h].workspaces[w].sessions.append(session)
        return session.id
    }

    func rename(_ sessionID: String, to title: String) {
        for h in hosts.indices {
            for w in hosts[h].workspaces.indices {
                if let s = hosts[h].workspaces[w].sessions.firstIndex(where: { $0.id == sessionID }) {
                    hosts[h].workspaces[w].sessions[s].title = title
                }
            }
        }
    }

    func addPairedHost(name: String) {
        hosts.append(Host(id: UUID().uuidString, name: name, isLaptop: false, status: .online, workspaces: [
            Workspace(id: UUID().uuidString, name: "home", path: "C:\\Users\\me", sessions: [
                Session(id: UUID().uuidString, title: "PowerShell", kind: .terminal, activity: .idle(since: nil), cwd: "C:\\Users\\me"),
            ]),
        ]))
    }

    func forget(_ hostID: String) {
        hosts.removeAll { $0.id == hostID }
    }
}
