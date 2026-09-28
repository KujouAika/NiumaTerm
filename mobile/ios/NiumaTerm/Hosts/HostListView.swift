import SwiftUI
import NiumaTermCore

/// Root: paired hosts, each a section listing its sessions (design doc §8.1).
struct HostListView: View {
    @Environment(AppModel.self) private var app
    @State private var newSessionHost: Host?
    @State private var pairing: PairingRequest?
    @State private var showSettings = false

    var body: some View {
        @Bindable var app = app
        NavigationStack(path: $app.path) {
            List {
                if let error = app.startupError {
                    Section {
                        Text(error).foregroundStyle(Theme.attention)
                    } header: {
                        Text("NiumaTerm could not start")
                    }
                }
                ForEach(app.hosts) { host in
                    Section {
                        ForEach(host.orderedSessions) { session in
                            NavigationLink(value: SessionRoute(hostID: host.id, sessionID: session.id, kind: session.kind)) {
                                SessionRow(session: session)
                            }
                            .listRowBackground(Theme.rowBackground)
                        }
                        if host.isOnline && host.sessions.isEmpty {
                            Text("No sessions. Tap + to start a terminal or an agent.")
                                .font(.system(size: 14))
                                .foregroundStyle(Theme.tertiary)
                                .listRowBackground(Theme.rowBackground)
                        }
                    } header: {
                        HostHeader(host: host, onNew: host.isOnline ? { newSessionHost = host } : nil)
                            .textCase(nil)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Theme.canvas)
            .overlay {
                if app.hosts.isEmpty && app.startupError == nil {
                    ContentUnavailableView {
                        Label("No computers yet", systemImage: "desktopcomputer")
                    } description: {
                        Text("Pair this phone with NiumaTerm on your computer to use its terminals and agents here.")
                    } actions: {
                        Button("Add computer") { pairing = PairingRequest(link: nil) }
                            .buttonStyle(.borderedProminent)
                    }
                }
            }
            .navigationTitle("Computers")
            .navigationDestination(for: SessionRoute.self) { SessionScreen(route: $0) }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Settings", systemImage: "gearshape") { showSettings = true }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Add computer", systemImage: "qrcode.viewfinder") { pairing = PairingRequest(link: nil) }
                }
            }
            .sheet(item: $newSessionHost) { host in
                NewSessionSheet(host: host)
            }
            .sheet(isPresented: $showSettings) {
                SettingsView()
            }
            .fullScreenCover(item: $pairing) { request in
                PairingView(initialLink: request.link)
            }
            // The Camera app hands a scanned niumaterm://pair link to the
            // app, which pairs as if it had scanned the code itself.
            .onOpenURL { url in
                guard url.scheme == "niumaterm", url.host() == "pair" else { return }
                pairing = PairingRequest(link: url.absoluteString)
            }
        }
    }
}

/// One presentation of the pairing screen. The link travels with it, so
/// the screen sees the link it was opened for rather than a value captured
/// when the cover was first declared.
struct PairingRequest: Identifiable {
    let id = UUID()
    let link: String?
}

struct HostHeader: View {
    let host: Host
    var onNew: (() -> Void)?

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: host.icon)
                .font(.system(size: 18))
                .foregroundStyle(Theme.ink)
            VStack(alignment: .leading, spacing: 2) {
                Text(host.name)
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(Theme.ink)
                HStack(spacing: 5) {
                    Circle().fill(host.statusColor).frame(width: 7, height: 7)
                    Text(host.statusText)
                        .font(.system(size: 12))
                        .foregroundStyle(Theme.secondary)
                }
            }
            Spacer()
            if let onNew {
                Button(action: onNew) {
                    Image(systemName: "plus")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundStyle(Theme.accent)
                        .frame(width: 32, height: 32)
                        .background(Theme.accent.opacity(0.1), in: .circle)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("New session on \(host.name)")
            }
        }
        .padding(.top, 8)
        .opacity(host.isOnline ? 1 : 0.7)
    }
}

struct SessionRow: View {
    let session: Session
    var body: some View {
        HStack(spacing: 12) {
            Text(session.glyph)
                .font(Theme.mono(13, weight: .medium))
                .foregroundStyle(Theme.ink2)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text(session.title)
                    .font(.system(size: 16))
                    .foregroundStyle(Theme.ink)
                    .lineLimit(1)
                Text(session.subtitle)
                    .font(.system(size: 12.5))
                    .foregroundStyle(Theme.secondary)
            }
            Spacer(minLength: 4)
        }
        .padding(.vertical, 3)
    }
}

/// Routes a session to its terminal or agent screen.
struct SessionScreen: View {
    @Environment(AppModel.self) private var app
    let route: SessionRoute

    var body: some View {
        let session = app.session(route)
        let hostName = app.host(route.hostID)?.name ?? "Computer"
        switch route.kind {
        case .agent:
            if let model = app.agentModel(for: route, session: session) {
                AgentSessionView(model: model, hostName: hostName)
                    .onDisappear { app.closeAgent(route) }
            }
        case .terminal:
            if let model = app.terminalModel(for: route, session: session) {
                TerminalSessionView(model: model, hostName: hostName)
                    .onDisappear { app.closeTerminal(route) }
            }
        case .other:
            ContentUnavailableView("Session ended", systemImage: "xmark.circle",
                                   description: Text("\(hostName) no longer lists this session."))
        }
    }
}
