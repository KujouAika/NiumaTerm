import SwiftUI

struct AgentSessionView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @Bindable var model: AgentSessionModel
    let hostName: String

    @FocusState private var composerFocused: Bool
    @State private var renaming = false
    @State private var newTitle = ""

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    if model.entries.isEmpty && !model.isWorking {
                        Text("Send a message to start.")
                            .font(.system(size: 15))
                            .foregroundStyle(Theme.tertiary)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 120)
                    }
                    ForEach(model.entries) { entry in
                        TranscriptRow(entry: entry)
                    }
                    if model.pending != nil && !model.showApproval {
                        Button { model.showApproval = true } label: {
                            Label("Review approval request", systemImage: "hand.raised")
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundStyle(Theme.attention)
                                .padding(.horizontal, 14)
                                .frame(height: 40)
                                .background(Theme.attention.opacity(0.1), in: .capsule)
                        }
                        .buttonStyle(.plain)
                    }
                    if model.isWorking {
                        WorkingRow(started: model.workStarted, tokens: model.tokens)
                    }
                    if model.interrupted {
                        Text("■ Interrupted by you")
                            .font(Theme.mono(12.5))
                            .foregroundStyle(Theme.attention)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
            }
            .defaultScrollAnchor(.bottom)
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: model.entries.count) {
                withAnimation(.snappy) { proxy.scrollTo("bottom", anchor: .bottom) }
            }
        }
        .background(Theme.transcriptBackground)
        .safeAreaInset(edge: .bottom) {
            ComposerView(model: model, focused: $composerFocused)
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(model.title).font(.system(size: 16, weight: .semibold)).lineLimit(1)
                    Text("\(model.profile.rawValue) · \(hostName)")
                        .font(.system(size: 11.5))
                        .foregroundStyle(Theme.secondary)
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("Rename", systemImage: "pencil") {
                        newTitle = model.title
                        renaming = true
                    }
                    Button("Rewind or fork…", systemImage: "arrow.uturn.backward") {}
                    Button("Tasks & goal", systemImage: "checklist") {}
                    Section("Demo") {
                        Button("Simulate approval request") { model.simulateApproval() }
                        Button("Simulate desktop taking control") { model.takenBack = true }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                }
            }
        }
        .alert("Rename session", isPresented: $renaming) {
            TextField("Title", text: $newTitle)
            Button("Cancel", role: .cancel) {}
            Button("Save") {
                model.title = newTitle
                app.rename(model.session.id, to: newTitle)
            }
        }
        .sheet(isPresented: $model.showApproval) {
            if let request = model.pending {
                ApprovalSheet(model: model, request: request)
                    .presentationDetents([.medium, .large])
            }
        }
        .sheet(isPresented: $model.takenBack) {
            TakenBackSheet(hostName: hostName, title: model.title,
                           onReconnect: { model.takenBack = false },
                           onClose: {
                               model.takenBack = false
                               Task {
                                   try? await Task.sleep(for: .milliseconds(300))
                                   dismiss()
                               }
                           })
                .presentationDetents([.height(400)])
                .interactiveDismissDisabled()
        }
    }
}

// MARK: Transcript rows

struct TranscriptRow: View {
    let entry: TranscriptEntry
    @AppStorage("transcriptMono") private var mono = true

    var body: some View {
        switch entry.kind {
        case .user(let text):
            Text(text)
                .font(.system(size: 15))
                .foregroundStyle(Theme.ink)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Theme.bubble, in: .rect(cornerRadius: 20))
                .frame(maxWidth: .infinity, alignment: .trailing)
                .padding(.leading, 48)
                .textSelection(.enabled)
        case .agent(let text):
            Text(markdown(text))
                .font(mono ? Theme.mono(14) : .system(size: 15))
                .lineSpacing(4)
                .foregroundStyle(Theme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
        case .thinking(let summary):
            DisclosureRowView(label: "Thinking", icon: "brain") {
                Text(summary)
                    .font(.system(size: 13.5))
                    .italic()
                    .foregroundStyle(Theme.secondary)
            }
        case .tools(let calls):
            DisclosureRowView(label: "+\(calls.count) tool calls", icon: nil) {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(calls) { call in
                        HStack(spacing: 8) {
                            Text(call.kind)
                                .foregroundStyle(Theme.accent)
                                .frame(minWidth: 40, alignment: .leading)
                            Text(call.detail)
                                .foregroundStyle(Theme.ink2)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer(minLength: 4)
                            Image(systemName: "checkmark")
                                .font(.system(size: 10, weight: .bold))
                                .foregroundStyle(Theme.online)
                        }
                        .font(Theme.mono(12))
                    }
                }
            }
        case .notice(let text, let color):
            Text(text)
                .font(Theme.mono(13))
                .foregroundStyle(color)
                .padding(.leading, 12)
                .overlay(alignment: .leading) { Rectangle().fill(Theme.rule).frame(width: 2) }
        }
    }

    private func markdown(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(s)
    }
}

/// The desktop's left-ruled collapsible row.
struct DisclosureRowView<Content: View>: View {
    let label: String
    let icon: String?
    @ViewBuilder var content: () -> Content
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                withAnimation(.snappy) { open.toggle() }
            } label: {
                HStack(spacing: 6) {
                    if let icon { Image(systemName: icon).font(.system(size: 12)) }
                    Text(label)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .font(Theme.mono(13))
                .foregroundStyle(Theme.secondary)
                .frame(minHeight: 28)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if open { content() }
        }
        .padding(.leading, 12)
        .overlay(alignment: .leading) { Rectangle().fill(Theme.rule).frame(width: 2) }
    }
}

struct WorkingRow: View {
    let started: Date
    let tokens: String

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let seconds = max(0, Int(context.date.timeIntervalSince(started)))
            HStack(spacing: 8) {
                PulsingDots()
                Text("Working for \(seconds / 60)m \(seconds % 60)s ·")
                    .foregroundStyle(Theme.secondary)
                Text("\(tokens) tokens")
                    .fontWeight(.semibold)
                    .foregroundStyle(Theme.ink)
            }
            .font(Theme.mono(12.5))
        }
    }
}

struct PulsingDots: View {
    var body: some View {
        TimelineView(.animation) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            HStack(spacing: 3) {
                ForEach(0..<3, id: \.self) { i in
                    Circle()
                        .fill(Theme.accent)
                        .frame(width: 4, height: 4)
                        .opacity(0.25 + 0.75 * (0.5 + 0.5 * sin((t - Double(i) * 0.2) * .pi * 2 / 1.2)))
                }
            }
        }
    }
}
