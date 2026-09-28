import SwiftUI

/// "+" on a host: New terminal, or a profile × workspace pair from `host.info` (§8.1).
struct NewSessionSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    let host: Host

    @State private var isAgent = true
    @State private var profile: AgentProfile = .claude
    @State private var workspaceID: String

    init(host: Host) {
        self.host = host
        _workspaceID = State(initialValue: host.workspaces.first?.id ?? "")
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    Picker("Kind", selection: $isAgent) {
                        Text(">_  Terminal").tag(false)
                        Text("✱  Agent").tag(true)
                    }
                    .pickerStyle(.segmented)

                    if isAgent {
                        VStack(alignment: .leading, spacing: 8) {
                            SectionLabel("Profile")
                            HStack(spacing: 8) {
                                ForEach(AgentProfile.allCases) { p in
                                    Button { profile = p } label: {
                                        Text(p.rawValue)
                                            .font(.system(size: 14, weight: .medium))
                                            .padding(.horizontal, 14)
                                            .frame(height: 36)
                                            .foregroundStyle(profile == p ? Color.white : Theme.ink)
                                            .background(profile == p ? Theme.accent : Color.black.opacity(0.06), in: .capsule)
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                        }
                        .transition(.opacity.combined(with: .move(edge: .top)))
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        SectionLabel("Workspace")
                        VStack(spacing: 0) {
                            ForEach(Array(host.workspaces.enumerated()), id: \.element.id) { index, ws in
                                if index > 0 { Divider().padding(.leading, 16) }
                                Button { workspaceID = ws.id } label: {
                                    HStack {
                                        VStack(alignment: .leading, spacing: 2) {
                                            Text(ws.name).font(.system(size: 16)).foregroundStyle(Theme.ink)
                                            Text(ws.path).font(Theme.mono(11.5)).foregroundStyle(Theme.tertiary)
                                        }
                                        Spacer()
                                        if workspaceID == ws.id {
                                            Image(systemName: "checkmark")
                                                .font(.system(size: 15, weight: .bold))
                                                .foregroundStyle(Theme.accent)
                                        }
                                    }
                                    .padding(.horizontal, 16)
                                    .frame(height: 56)
                                    .contentShape(.rect)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                        .background(Color.white, in: .rect(cornerRadius: 22))
                    }
                }
                .padding(20)
                .animation(.snappy, value: isAgent)
            }
            .safeAreaInset(edge: .bottom) {
                Button(isAgent ? "Start \(profile.rawValue)" : "Open Terminal", action: create)
                    .buttonStyle(PrimaryButtonStyle())
                    .padding(.horizontal, 20)
                    .padding(.bottom, 8)
            }
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .principal) {
                    VStack(spacing: 1) {
                        Text("New Session").font(.system(size: 17, weight: .semibold))
                        Text(host.name).font(.system(size: 12)).foregroundStyle(Theme.secondary)
                    }
                }
            }
        }
    }

    private func create() {
        let kind: SessionKind = isAgent ? .agent(profile) : .terminal
        guard let id = app.createSession(hostID: host.id, workspaceID: workspaceID, kind: kind) else { return }
        dismiss()
        Task {
            try? await Task.sleep(for: .milliseconds(350))
            app.path.append(id)
        }
    }
}
