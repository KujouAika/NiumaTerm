import SwiftUI

/// Root: paired hosts, each listing its sessions grouped by workspace (design doc §8.1).
struct HostListView: View {
    @Environment(AppModel.self) private var app
    @State private var newSessionHost: Host?
    @State private var showPairing = false
    @State private var showSettings = false

    var body: some View {
        @Bindable var app = app
        NavigationStack(path: $app.path) {
            List {
                ForEach(app.hosts) { host in
                    if host.isOnline && !host.workspaces.isEmpty {
                        ForEach(Array(host.workspaces.enumerated()), id: \.element.id) { index, ws in
                            Section {
                                ForEach(ws.sessions) { session in
                                    NavigationLink(value: session.id) {
                                        SessionRow(session: session)
                                    }
                                    .listRowBackground(Theme.rowBackground)
                                }
                            } header: {
                                VStack(alignment: .leading, spacing: 14) {
                                    if index == 0 {
                                        HostHeader(host: host) { newSessionHost = host }
                                    }
                                    WorkspaceHeader(workspace: ws)
                                }
                                .textCase(nil)
                            }
                        }
                    } else {
                        Section {
                            EmptyView()
                        } header: {
                            HostHeader(host: host, onNew: nil).textCase(nil)
                        }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Theme.canvas)
            .navigationTitle("Computers")
            .navigationDestination(for: String.self) { SessionScreen(sessionID: $0) }
            .refreshable { try? await Task.sleep(for: .milliseconds(600)) }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Settings", systemImage: "gearshape") { showSettings = true }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Add computer", systemImage: "qrcode.viewfinder") { showPairing = true }
                }
            }
            .sheet(item: $newSessionHost) { host in
                NewSessionSheet(host: host)
            }
            .sheet(isPresented: $showSettings) {
                SettingsView()
            }
            .fullScreenCover(isPresented: $showPairing) {
                PairingView()
            }
        }
    }
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

struct WorkspaceHeader: View {
    let workspace: Workspace
    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(workspace.name)
                .font(.system(size: 12, weight: .semibold))
                .textCase(.uppercase)
                .tracking(0.4)
            Text(workspace.shortPath)
                .font(Theme.mono(11))
                .foregroundStyle(Theme.tertiary)
        }
        .foregroundStyle(Theme.secondary)
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
                Text(session.statusText)
                    .font(.system(size: 12.5))
                    .foregroundStyle(session.statusColor)
            }
            Spacer(minLength: 4)
            if session.controlledOnDesktop {
                Image(systemName: "desktopcomputer")
                    .font(.system(size: 13))
                    .foregroundStyle(Theme.tertiary)
                    .accessibilityLabel("Controlled on desktop")
            }
        }
        .padding(.vertical, 3)
    }
}

/// Routes a session id to its terminal or agent screen.
struct SessionScreen: View {
    @Environment(AppModel.self) private var app
    let sessionID: String

    var body: some View {
        if let found = app.lookup(sessionID) {
            switch found.session.kind {
            case .terminal:
                TerminalSessionView(model: app.terminalModel(for: found.session),
                                    workspaceName: found.workspace.name)
            case .agent:
                AgentSessionView(model: app.agentModel(for: found.session),
                                 hostName: found.host.name)
            }
        } else {
            ContentUnavailableView("Session ended", systemImage: "xmark.circle",
                                   description: Text("The host closed this session."))
        }
    }
}
