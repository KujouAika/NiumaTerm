import SwiftUI
import NiumaTermCore

/// Approval bottom sheet driven by the `pending` slot.
struct ApprovalSheet: View {
    @Bindable var model: AgentSessionModel
    let approval: PendingApproval

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Text(model.profile?.glyph ?? "✱")
                    .font(Theme.mono(16, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                Text("\(model.profile?.shortName ?? "Agent") needs approval")
                    .font(.system(size: 19, weight: .semibold))
            }

            ScrollView {
                Text(approval.description)
                    .font(Theme.mono(12.5))
                    .foregroundStyle(Theme.codeText)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
                    .padding(14)
            }
            .frame(maxHeight: 260)
            .fixedSize(horizontal: false, vertical: true)
            .background(Theme.codeBackground, in: .rect(cornerRadius: 18))
            .padding(.top, 16)

            Toggle("Allow for this session", isOn: $model.allowForSession)
                .font(.system(size: 15))
                .tint(Theme.accent)
                .padding(.horizontal, 14)
                .frame(height: 52)
                .background(Theme.card, in: .rect(cornerRadius: 18))
                .padding(.top, 16)

            Spacer(minLength: 16)

            HStack(spacing: 10) {
                Button("Deny") { model.deny() }
                    .buttonStyle(SecondaryButtonStyle())
                Button("Approve") { model.approve() }
                    .buttonStyle(PrimaryButtonStyle())
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 28)
        .padding(.bottom, 12)
        .sensoryFeedback(.warning, trigger: approval.description)
    }
}

/// Questions the agent asked, answered together: pick options, or type an
/// answer where a question takes text.
struct QuestionSheet: View {
    @Bindable var model: AgentSessionModel
    let batch: QuestionBatch

    @AppStorage(ComposerSettings.questionsOneAtATimeKey) private var oneAtATime = false

    /// The question shown when the batch is answered one at a time.
    @State private var page = 0

    /// The one question on screen, or nil when the sheet lists them all.
    private var shown: Int? {
        guard oneAtATime, batch.questions.count > 1 else { return nil }
        return min(page, batch.questions.count - 1)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Text(model.profile?.glyph ?? "✱")
                    .font(Theme.mono(16, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                Text(batch.questions.count == 1 ? "\(model.agentName) asks" : "\(model.agentName) asks \(batch.questions.count) questions")
                    .font(.system(size: 19, weight: .semibold))
            }

            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    ForEach(Array(batch.questions.enumerated()), id: \.offset) { index, item in
                        if shown == nil || shown == index {
                            QuestionCard(model: model, batch: batch, index: index, item: item)
                        }
                    }
                }
                .padding(.vertical, 16)
            }

            if let skipsAt = model.questionSkipsAt, skipsAt > .now {
                (Text("Skipped automatically in ") + Text(timerInterval: Date.now...skipsAt, countsDown: true))
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.secondary)
                    .padding(.bottom, 8)
            }

            if let notice = model.questionNotice ?? batch.error {
                Text(notice)
                    .font(Theme.mono(12.5))
                    .foregroundStyle(Theme.attention)
                    .padding(.bottom, 8)
            }

            if let shown {
                Text("Question \(shown + 1) of \(batch.questions.count)")
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.secondary)
                    .padding(.bottom, 8)
            }

            HStack(spacing: 10) {
                Button("Skip") { model.skipQuestions(batch) }
                    .buttonStyle(SecondaryButtonStyle())
                if let shown, shown > 0 {
                    Button("Back") { page = shown - 1 }
                        .buttonStyle(SecondaryButtonStyle())
                }
                if let shown, shown + 1 < batch.questions.count {
                    let answered = model.isAnswered(batch, shown)
                    Button("Next") { page = shown + 1 }
                        .buttonStyle(PrimaryButtonStyle())
                        .disabled(!answered)
                        .opacity(answered ? 1 : 0.45)
                } else {
                    Button(batch.submitting ? "Sending…" : "Answer") { model.submitQuestions(batch) }
                        .buttonStyle(PrimaryButtonStyle())
                        .disabled(batch.submitting || !model.isComplete(batch))
                        .opacity(batch.submitting || !model.isComplete(batch) ? 0.45 : 1)
                }
            }
        }
        // A new batch starts from its first question.
        .onChange(of: batch.id) { page = 0 }
        .padding(.horizontal, 20)
        .padding(.top, 28)
        .padding(.bottom, 12)
        .sensoryFeedback(.warning, trigger: batch.id)
    }
}

private struct QuestionCard: View {
    @Bindable var model: AgentSessionModel
    let batch: QuestionBatch
    let index: Int
    let item: QuestionItem

    var body: some View {
        let answer = model.answer(batch, index)
        VStack(alignment: .leading, spacing: 10) {
            if let header = item.header, !header.isEmpty {
                Text(header.uppercased())
                    .font(Theme.mono(10.5, weight: .semibold))
                    .foregroundStyle(Theme.accent)
            }
            Text(item.question)
                .font(.system(size: 16, weight: .medium))
                .fixedSize(horizontal: false, vertical: true)

            VStack(spacing: 0) {
                ForEach(Array(item.options.enumerated()), id: \.offset) { option, choice in
                    let picked = answer.text == nil && answer.selected.contains(UInt32(option))
                    Button { model.toggle(batch, question: index, option: option) } label: {
                        HStack(alignment: .top, spacing: 12) {
                            Image(systemName: picked
                                  ? (item.multiSelect ? "checkmark.square.fill" : "largecircle.fill.circle")
                                  : (item.multiSelect ? "square" : "circle"))
                                .foregroundStyle(picked ? Theme.accent : Theme.tertiary)
                                .font(.system(size: 17))
                            VStack(alignment: .leading, spacing: 2) {
                                Text(choice.label)
                                    .font(.system(size: 15))
                                    .foregroundStyle(Theme.ink)
                                if let description = choice.description, !description.isEmpty {
                                    Text(description)
                                        .font(.system(size: 13))
                                        .foregroundStyle(Theme.secondary)
                                }
                            }
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 11)
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .disabled(batch.submitting)
                }

                if item.input != .selectionOnly || item.options.isEmpty {
                    let text = Binding(
                        get: { answer.text ?? "" },
                        set: { model.setText(batch, question: index, text: $0) }
                    )
                    let prompt = item.options.isEmpty ? "Your answer" : "Or type your own answer"
                    Group {
                        if item.input == .secret {
                            SecureField(prompt, text: text)
                        } else {
                            TextField(prompt, text: text, axis: .vertical)
                                .lineLimit(1...4)
                        }
                    }
                    .font(.system(size: 15))
                    .padding(.horizontal, 14)
                    .padding(.vertical, 11)
                    .disabled(batch.submitting)
                }
            }
            // A tint rather than the card color: the sheet grows to a plain
            // background, against which a card color would vanish.
            .background(Theme.fill, in: .rect(cornerRadius: 18))
        }
    }
}

/// Presents `EndedSheet` for as long as a view stays ended.
struct EndedSheetItem: Identifiable {
    let end: ViewEnd
    var id: ViewEnd { end }
}

/// `session.ended` and a lost host (§8.4). Taken back offers Reconnect,
/// which takes control again; a closed session only closes.
struct EndedSheet: View {
    let end: ViewEnd
    let hostName: String
    let title: String
    var onReconnect: () -> Void
    var onClose: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Image(systemName: icon)
                .font(.system(size: 24))
                .foregroundStyle(Theme.accent)
                .frame(width: 56, height: 56)
                .background(Theme.accent.opacity(0.12), in: .circle)
                .padding(.bottom, 14)
            Text(headline)
                .font(.system(size: 19, weight: .semibold))
                .multilineTextAlignment(.center)
                .padding(.bottom, 6)
            Text(message)
                .font(.system(size: 14.5))
                .foregroundStyle(Theme.secondary)
                .multilineTextAlignment(.center)
                .padding(.bottom, 22)
            VStack(spacing: 10) {
                if end == .takenBack {
                    Button("Reconnect", action: onReconnect).buttonStyle(PrimaryButtonStyle())
                    Button("Close", action: onClose).buttonStyle(SecondaryButtonStyle())
                } else {
                    Button("Close", action: onClose).buttonStyle(PrimaryButtonStyle())
                }
            }
        }
        .padding(.horizontal, 22)
        .padding(.top, 28)
    }

    private var icon: String {
        switch end {
        case .takenBack: "desktopcomputer"
        case .closed: "xmark.circle"
        case .unreachable: "wifi.slash"
        }
    }

    private var headline: String {
        switch end {
        case .takenBack: "\(hostName) took this session back"
        case .closed: "This session ended"
        case .unreachable: "\(hostName) is out of reach"
        }
    }

    private var message: String {
        switch end {
        case .takenBack: "“\(title)” is controlled on the desktop now. Reconnect to take control again."
        case .closed: "“\(title)” was closed on \(hostName)."
        case .unreachable: "This phone can no longer reach \(hostName). If it was removed there, pair again."
        }
    }
}
