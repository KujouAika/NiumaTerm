import SwiftUI
import NiumaTermCore

/// Root: paired hosts, each a section listing its sessions (design doc §8.1).
struct HostListView: View {
    @Environment(AppModel.self) private var app
    @State private var newSessionHost: Host?
    @State private var pairing: PairingRequest?
    @State private var showSettings = false
    @State private var pairAfterSettings = false
    @State private var closing: SessionClose?
    @State private var closeError: String?

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
                    let groups = host.sessionGroups
                    let header = HostHeader(host: host,
                                            onNew: host.isOnline ? { newSessionHost = host } : nil,
                                            onRetry: host.status == .unreachable ? { app.retry(host.id) } : nil)
                        .textCase(nil)
                    if groups.isEmpty {
                        Section {
                            if host.isOnline {
                                Text("No sessions. Tap + to start a terminal or an agent.")
                                    .font(.system(size: 14))
                                    .foregroundStyle(Theme.tertiary)
                                    .listRowBackground(Theme.rowBackground)
                            }
                        } header: {
                            header
                        }
                    }
                    // One section per workspace; the first also carries the
                    // host's header, so each host still reads as one block.
                    ForEach(groups) { group in
                        Section {
                            ForEach(group.sessions) { session in
                                let route = SessionRoute(hostID: host.id, sessionID: session.id, kind: session.kind)
                                NavigationLink(value: route) {
                                    SessionRow(session: session)
                                }
                                .listRowBackground(Theme.rowBackground)
                                // No destructive role: SwiftUI would slide the
                                // row out at once, before the person confirms
                                // or the host agrees to close the session.
                                .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                    if host.isOnline {
                                        Button("Close", systemImage: "xmark") {
                                            closing = SessionClose(route: route, title: session.title,
                                                                   hostName: host.name)
                                        }
                                        .tint(.red)
                                    }
                                }
                            }
                        } header: {
                            VStack(alignment: .leading, spacing: 10) {
                                if group.id == groups.first?.id {
                                    header
                                }
                                if groups.count > 1 || group.workspace != nil {
                                    WorkspaceHeader(name: group.workspace?.name ?? "Other")
                                }
                            }
                        }
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
            }
            .confirmationDialog(
                "Close “\(closing?.title ?? "")”?",
                isPresented: Binding(get: { closing != nil }, set: { if !$0 { closing = nil } }),
                titleVisibility: .visible,
                presenting: closing
            ) { request in
                Button("Close Session", role: .destructive) { close(request) }
            } message: { request in
                Text("The session ends on \(request.hostName), along with anything running in it.")
            }
            .alert(
                "Could not close the session",
                isPresented: Binding(get: { closeError != nil }, set: { if !$0 { closeError = nil } }),
                presenting: closeError
            ) { _ in
                Button("OK", role: .cancel) {}
            } message: { message in
                Text(message)
            }
            .sheet(item: $newSessionHost) { host in
                NewSessionSheet(host: host)
            }
            // Pairing opens only once Settings has gone, so the two screens
            // never try to present at the same time.
            .sheet(isPresented: $showSettings, onDismiss: {
                if pairAfterSettings {
                    pairAfterSettings = false
                    pairing = PairingRequest(link: nil)
                }
            }) {
                SettingsView(onAddTarget: {
                    pairAfterSettings = true
                    showSettings = false
                })
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

    private func close(_ request: SessionClose) {
        Task {
            do {
                try await app.closeSession(request.route)
            } catch {
                closeError = error.displayText
            }
        }
    }
}

/// A session the person asked to close, held while they confirm.
struct SessionClose {
    let route: SessionRoute
    let title: String
    let hostName: String
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
    var onRetry: (() -> Void)?

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
            if let onRetry {
                Button("Retry", action: onRetry)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                    .buttonStyle(.plain)
                    .accessibilityLabel("Retry connecting to \(host.name)")
            }
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

/// Names the host workspace a group of sessions comes from.
struct WorkspaceHeader: View {
    let name: String

    var body: some View {
        Label(name, systemImage: "folder")
            .font(.system(size: 12.5, weight: .medium))
            .foregroundStyle(Theme.secondary)
            .textCase(nil)
            .lineLimit(1)
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
