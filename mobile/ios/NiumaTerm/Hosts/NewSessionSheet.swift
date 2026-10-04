import SwiftUI

import NiumaTermCore

/// "+" on a host: a terminal, or an agent from a profile × workspace pair
/// in `host.info` (§8.1).
struct NewSessionSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    let host: Host

    @State private var offer: HostOffer?
    @State private var loadError: String?
    @State private var profile: String?
    @State private var workspace: String?
    @State private var starting = false
    @State private var startError: String?
    @State private var openingTerminal = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    terminalChoice

                    if let offer {
                        form(offer)
                    } else if let loadError {
                        Text(loadError)
                            .foregroundStyle(Theme.attention)
                    } else {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                            .padding(.top, 60)
                    }

                    if let startError {
                        Text(startError)
                            .font(.system(size: 14))
                            .foregroundStyle(Theme.attention)
                    }
                }
                .padding(20)
            }
            .safeAreaInset(edge: .bottom) {
                Button(action: create) {
                    HStack(spacing: 8) {
                        if starting { ProgressView().tint(Theme.onAccent) }

                        Text(startLabel)
                    }
                }
                .buttonStyle(PrimaryButtonStyle())
                .disabled(profile == nil || workspace == nil || starting)
                .opacity(profile == nil || workspace == nil ? 0.5 : 1)
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
            .task { await load() }
        }
    }

    private var startLabel: String {
        guard let profile else { return tr("Start agent") }

        return tr("Start \(profile)")
    }

    private var terminalChoice: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel("Terminal")

            Button(action: openTerminal) {
                HStack(spacing: 12) {
                    Text(">_")
                        .font(Theme.mono(14, weight: .semibold))
                        .foregroundStyle(Theme.ink2)

                    VStack(alignment: .leading, spacing: 2) {
                        Text("New terminal").font(.system(size: 16)).foregroundStyle(Theme.ink)

                        Text("The default shell on \(host.name)")
                            .font(.system(size: 12.5))
                            .foregroundStyle(Theme.tertiary)
                    }

                    Spacer()

                    if openingTerminal {
                        ProgressView()
                    } else {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(Theme.tertiary)
                    }
                }
                .padding(.horizontal, 16)
                .frame(height: 60)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .disabled(openingTerminal || starting)
            .background(Theme.card, in: .rect(cornerRadius: 22))
        }
    }

    private func openTerminal() {
        openingTerminal = true
        startError = nil

        Task {
            do {
                let route = try await app.openTerminal(hostID: host.id)

                dismiss()

                try? await Task.sleep(for: .milliseconds(350))
                app.path.append(route)
            } catch {
                startError = error.displayText
                openingTerminal = false
            }
        }
    }

    @ViewBuilder
    private func form(_ offer: HostOffer) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel("Profile")

            if offer.agents.isEmpty {
                Text("\(host.name) has no agent profiles.")
                    .foregroundStyle(Theme.secondary)
            }

            FlowChips(items: offer.agents.map(\.name), selection: $profile) { name in
                let harness = offer.agents.first { $0.name == name }?.harness ?? ""

                return "\(AgentProfile(harness: harness).glyph)  \(name)"
            }
        }

        VStack(alignment: .leading, spacing: 8) {
            SectionLabel("Workspace")

            if offer.workspaces.isEmpty {
                Text("Open a workspace on \(host.name) first; agents start only where you already work.")
                    .foregroundStyle(Theme.secondary)
            }

            VStack(spacing: 0) {
                ForEach(Array(offer.workspaces.enumerated()), id: \.element.path) { index, ws in
                    if index > 0 { Divider().padding(.leading, 16) }

                    Button { workspace = ws.path } label: {
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(ws.name).font(.system(size: 16)).foregroundStyle(Theme.ink)

                                Text(ws.path).font(Theme.mono(11.5)).foregroundStyle(Theme.tertiary)
                                    .lineLimit(1).truncationMode(.head)
                            }

                            Spacer()

                            if workspace == ws.path {
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
            .background(Theme.card, in: .rect(cornerRadius: 22))
        }
    }

    private func load() async {
        do {
            let offer = try await app.hostOffer(host.id)

            self.offer = offer
            profile = offer.agents.first?.name
            workspace = offer.workspaces.first?.path
        } catch {
            loadError = error.displayText
        }
    }

    private func create() {
        guard let profile, let workspace else { return }

        starting = true
        startError = nil

        Task {
            do {
                let route = try await app.openAgent(hostID: host.id, profile: profile, workspace: workspace)

                dismiss()

                try? await Task.sleep(for: .milliseconds(350))
                app.path.append(route)
            } catch {
                startError = error.displayText
                starting = false
            }
        }
    }
}

/// Selectable capsules that wrap onto new lines.
struct FlowChips: View {
    let items: [String]
    @Binding var selection: String?
    var label: (String) -> String

    var body: some View {
        FlowLayout(spacing: 8) {
            ForEach(items, id: \.self) { item in
                Button { selection = item } label: {
                    Text(label(item))
                        .font(.system(size: 14, weight: .medium))
                        .padding(.horizontal, 14)
                        .frame(height: 36)
                        .foregroundStyle(selection == item ? Theme.onAccent : Theme.ink)
                        .background(selection == item ? Theme.accent : Theme.fill, in: .capsule)
                }
                .buttonStyle(.plain)
            }
        }
    }
}

struct FlowLayout: Layout {
    var spacing: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(width: proposal.width ?? .infinity, subviews: subviews)
        let height = rows.last.map { $0.y + $0.height } ?? 0

        return CGSize(width: proposal.width ?? rows.map(\.width).max() ?? 0, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let rows = arrange(width: bounds.width, subviews: subviews)

        for row in rows {
            var x = bounds.minX

            for index in row.indices {
                let size = subviews[index].sizeThatFits(.unspecified)

                subviews[index].place(at: CGPoint(x: x, y: bounds.minY + row.y), proposal: ProposedViewSize(size))

                x += size.width + spacing
            }
        }
    }

    private struct Row {
        var indices: [Int] = []
        var y: CGFloat = 0
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func arrange(width: CGFloat, subviews: Subviews) -> [Row] {
        var rows: [Row] = [Row()]

        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(.unspecified)

            var row = rows[rows.count - 1]

            if !row.indices.isEmpty && row.width + spacing + size.width > width {
                let y = row.y + row.height + spacing

                rows.append(Row(y: y))

                row = rows[rows.count - 1]
            }

            row.width += (row.indices.isEmpty ? 0 : spacing) + size.width
            row.height = max(row.height, size.height)

            row.indices.append(index)

            rows[rows.count - 1] = row
        }

        return rows
    }
}
