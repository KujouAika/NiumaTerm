import SwiftUI
import Observation
import UIKit
import NiumaTermCore

/// App-wide state over `MobileCore` (design doc §4.2): paired hosts, their
/// live session lists, and the open agent and terminal views.
@MainActor
@Observable
final class AppModel {
    var hosts: [Host] = []
    var path = NavigationPath()

    /// Why the core could not start, which leaves the app unable to pair.
    var startupError: String?

    @ObservationIgnored private var core: MobileCore?
    @ObservationIgnored private var events: CoreEvents?
    @ObservationIgnored private var agentModels: [String: AgentSessionModel] = [:]
    @ObservationIgnored private var terminalModels: [String: TerminalSessionModel] = [:]

    init() {
        do {
            let core = try MobileCore(stateDir: Self.stateDirectory().path,
                                      deviceName: UIDevice.current.name,
                                      appVersion: Self.appVersion)
            self.core = core
            hosts = core.hosts().map { Host(id: $0.id, name: $0.name, status: $0.status) }

            let events = CoreEvents(app: self)
            self.events = events
            core.observe(observer: events)
        } catch {
            startupError = error.displayText
        }
    }

    static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
    }

    /// Pairing records and the sealed device key live in Application Support,
    /// which is backed up but never shown to the user.
    private static func stateDirectory() throws -> URL {
        let base = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                               appropriateFor: nil, create: true)
        let dir = base.appendingPathComponent("remote", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    // MARK: Lifecycle

    func setForeground(_ foreground: Bool) {
        core?.setForeground(foreground: foreground)
    }

    // MARK: Core events

    func hostChanged(_ record: HostRecord) {
        if let index = hosts.firstIndex(where: { $0.id == record.id }) {
            hosts[index].name = record.name
            hosts[index].status = record.status
        } else {
            hosts.append(Host(id: record.id, name: record.name, status: record.status))
        }
    }

    func sessionsChanged(host: String, sessions: [SessionRecord]) {
        guard let index = hosts.firstIndex(where: { $0.id == host }) else { return }
        hosts[index].sessions = sessions.map { Session(record: $0, hostID: host) }
    }

    // MARK: Lookups

    func host(_ id: String) -> Host? {
        hosts.first { $0.id == id }
    }

    func session(_ route: SessionRoute) -> Session? {
        host(route.hostID)?.sessions.first { $0.id == route.sessionID }
    }

    // MARK: Commands

    func pair(_ input: String, relayURL: String? = nil, accessKey: String? = nil) async throws -> HostRecord {
        guard let core else { throw CoreError.Failed(message: startupError ?? "The app could not start its core.") }
        let record = try await core.pair(input: input, relayUrl: relayURL, accessKey: accessKey)
        hostChanged(record)
        return record
    }

    func forget(_ hostID: String) {
        core?.forget(host: hostID)
        hosts.removeAll { $0.id == hostID }
        for key in agentModels.keys where key.hasPrefix(hostID + "/") {
            agentModels[key] = nil
        }
        for key in terminalModels.keys where key.hasPrefix(hostID + "/") {
            terminalModels[key] = nil
        }
    }

    func hostOffer(_ hostID: String) async throws -> HostOffer {
        guard let core else { throw CoreError.Failed(message: "The app could not start its core.") }
        return try await core.hostInfo(host: hostID)
    }

    /// `agent.open`; returns the route of the new session.
    func openAgent(hostID: String, profile: String, workspace: String) async throws -> SessionRoute {
        guard let core else { throw CoreError.Failed(message: "The app could not start its core.") }
        let session = try await core.openAgent(host: hostID, profile: profile, workspace: workspace)
        return SessionRoute(hostID: hostID, sessionID: session, kind: .agent)
    }

    /// Attaching takes control of the session from the desktop (§8.4). The
    /// model stays cached while the screen is open, so the view survives
    /// navigation redraws; `closeAgent` detaches.
    func agentModel(for route: SessionRoute, session: Session?) -> AgentSessionModel? {
        let key = "\(route.hostID)/\(route.sessionID)"
        if let model = agentModels[key] { return model }
        guard let core else { return nil }
        let model = AgentSessionModel(core: core, route: route,
                                      title: session?.title ?? "Agent",
                                      profile: session?.profile)
        agentModels[key] = model
        return model
    }

    /// Drop the view of a session, which detaches from it on the host.
    func closeAgent(_ route: SessionRoute) {
        agentModels["\(route.hostID)/\(route.sessionID)"]?.detach()
        agentModels["\(route.hostID)/\(route.sessionID)"] = nil
    }

    /// Start a shell on the host and return the route of its screen, whose
    /// view is already attached.
    func openTerminal(hostID: String) async throws -> SessionRoute {
        guard let core else { throw CoreError.Failed(message: "The app could not start its core.") }
        let grid = TerminalMetrics.estimatedGrid()
        let events = TerminalEvents()
        let handle = try await core.openTerminal(host: hostID, cols: UInt16(grid.cols), rows: UInt16(grid.rows),
                                                 observer: events)
        let route = SessionRoute(hostID: hostID, sessionID: handle.session(), kind: .terminal)
        terminalModels["\(route.hostID)/\(route.sessionID)"] = TerminalSessionModel(handle: handle, events: events,
                                                                                     route: route)
        return route
    }

    /// Attaching takes control of the session from the desktop (§8.4), as
    /// for agents; `closeTerminal` detaches.
    func terminalModel(for route: SessionRoute, session: Session?) -> TerminalSessionModel? {
        let key = "\(route.hostID)/\(route.sessionID)"
        if let model = terminalModels[key] { return model }
        guard let core else { return nil }
        let model = TerminalSessionModel(core: core, route: route, title: session?.title ?? "Terminal")
        terminalModels[key] = model
        return model
    }

    /// Drop the view of a terminal, which detaches from it on the host; the
    /// shell keeps running there.
    func closeTerminal(_ route: SessionRoute) {
        terminalModels["\(route.hostID)/\(route.sessionID)"]?.detach()
        terminalModels["\(route.hostID)/\(route.sessionID)"] = nil
    }
}
