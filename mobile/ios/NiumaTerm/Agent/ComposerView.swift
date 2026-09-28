import SwiftUI
import NiumaTermCore

/// Floating glass composer: queue, text field, pickers from the `settings` slot, Send/Interrupt.
struct ComposerView: View {
    @Bindable var model: AgentSessionModel
    var focused: FocusState<Bool>.Binding

    var body: some View {
        VStack(spacing: 8) {
            ForEach(Array(model.queue.enumerated()), id: \.offset) { _, message in
                HStack(spacing: 8) {
                    Text("QUEUED")
                        .font(Theme.mono(10.5, weight: .semibold))
                        .foregroundStyle(Theme.accent)
                    Text(message.text)
                        .font(.system(size: 13))
                        .foregroundStyle(Theme.ink2)
                        .lineLimit(1)
                    Spacer()
                    if message.id != nil {
                        Button { model.withdraw(message) } label: {
                            Image(systemName: "xmark")
                                .font(.system(size: 10, weight: .bold))
                                .foregroundStyle(Theme.ink2)
                                .frame(width: 26, height: 26)
                                .background(Theme.fill, in: .circle)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Withdraw")
                    }
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
                    .disabled(model.ended != nil)
                HStack(spacing: 6) {
                    if !model.models.isEmpty {
                        Menu {
                            ForEach(model.models, id: \.model) { choice in
                                Button {
                                    model.selectModel(choice.model)
                                } label: {
                                    if choice.model == model.model {
                                        Label(choice.display, systemImage: "checkmark")
                                    } else {
                                        Text(choice.display)
                                    }
                                }
                            }
                        } label: { ChipLabel(text: model.modelLabel) }
                    }

                    if !model.efforts.isEmpty {
                        Menu {
                            ForEach(model.efforts, id: \.self) { effort in
                                Button {
                                    model.selectEffort(effort)
                                } label: {
                                    if effort == model.effort {
                                        Label(effort.capitalized, systemImage: "checkmark")
                                    } else {
                                        Text(effort.capitalized)
                                    }
                                }
                            }
                        } label: { ChipLabel(text: model.effort?.capitalized ?? "Effort") }
                    }

                    Spacer()

                    Button(action: model.primaryAction) {
                        Image(systemName: model.showsStop ? "stop.fill" : "arrow.up")
                            .font(.system(size: model.showsStop ? 12 : 15, weight: .bold))
                            .foregroundStyle(Theme.onAccent)
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
                Text(model.phaseLine).lineLimit(1)
                Spacer()
                if let context = model.contextLine { Text(context) }
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
            Text(text).lineLimit(1)
            Image(systemName: "chevron.down")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(Theme.tertiary)
        }
        .font(Theme.mono(12.5))
        .foregroundStyle(Theme.ink)
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(Theme.fill, in: .capsule)
    }
}
