import SwiftUI

/// Floating glass composer: queue, text field, pickers from the `settings` slot, Send/Interrupt.
struct ComposerView: View {
    @Bindable var model: AgentSessionModel
    var focused: FocusState<Bool>.Binding

    var body: some View {
        VStack(spacing: 8) {
            ForEach(model.queue) { q in
                HStack(spacing: 8) {
                    Text("QUEUED")
                        .font(Theme.mono(10.5, weight: .semibold))
                        .foregroundStyle(Theme.accent)
                    Text(q.text)
                        .font(.system(size: 13))
                        .foregroundStyle(Theme.ink2)
                        .lineLimit(1)
                    Spacer()
                    Button { withAnimation(.snappy) { model.withdraw(q) } } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundStyle(Theme.ink2)
                            .frame(width: 26, height: 26)
                            .background(Color.black.opacity(0.06), in: .circle)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Withdraw")
                }
                .padding(.leading, 14)
                .padding(.trailing, 8)
                .padding(.vertical, 6)
                .glassRounded(16)
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }

            VStack(alignment: .leading, spacing: 10) {
                TextField("Message \(model.agentName)", text: $model.draft, axis: .vertical)
                    .lineLimit(1...6)
                    .font(.system(size: 15))
                    .focused(focused)
                HStack(spacing: 6) {
                    Button {} label: {
                        Image(systemName: "plus")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(Theme.ink2)
                            .frame(width: 32, height: 32)
                            .background(Color.black.opacity(0.05), in: .circle)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Attach image")

                    Menu {
                        Picker("Model", selection: $model.model) {
                            ForEach(model.profile.models, id: \.self) { Text($0) }
                        }
                    } label: { ChipLabel(text: model.model) }

                    Menu {
                        Picker("Effort", selection: $model.effort) {
                            ForEach(model.efforts, id: \.self) { Text($0) }
                        }
                    } label: { ChipLabel(text: model.effort) }

                    Spacer()

                    Button(action: model.primaryAction) {
                        Image(systemName: model.showsStop ? "stop.fill" : "arrow.up")
                            .font(.system(size: model.showsStop ? 12 : 15, weight: .bold))
                            .foregroundStyle(Color.white)
                            .frame(width: 36, height: 36)
                            .background(Theme.accent.opacity(model.primaryEnabled ? 1 : 0.35), in: .circle)
                            .contentTransition(.symbolEffect(.replace))
                    }
                    .buttonStyle(.plain)
                    .disabled(!model.primaryEnabled)
                    .accessibilityLabel(model.showsStop ? "Interrupt" : "Send")
                }
            }
            .padding(.leading, 16)
            .padding(.trailing, 10)
            .padding(.top, 12)
            .padding(.bottom, 10)
            .glassRounded(28)

            HStack {
                Label(model.branch, systemImage: "arrow.triangle.branch")
                Spacer()
                Text(model.contextLine)
            }
            .font(Theme.mono(10.5))
            .foregroundStyle(Theme.secondary)
            .padding(.horizontal, 10)
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 4)
        .animation(.snappy, value: model.queue.count)
    }
}

struct ChipLabel: View {
    let text: String
    var body: some View {
        HStack(spacing: 4) {
            Text(text)
            Image(systemName: "chevron.down")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(Theme.tertiary)
        }
        .font(Theme.mono(12.5))
        .foregroundStyle(Theme.ink)
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(Color.black.opacity(0.05), in: .capsule)
    }
}
