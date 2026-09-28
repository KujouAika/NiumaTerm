import SwiftUI

/// Approval bottom sheet driven by the `pending` slot.
struct ApprovalSheet: View {
    @Bindable var model: AgentSessionModel
    let request: ApprovalRequest

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Text(model.profile.glyph)
                    .font(Theme.mono(16, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                Text(request.title)
                    .font(.system(size: 19, weight: .semibold))
            }
            Text(request.summary)
                .font(.system(size: 14))
                .foregroundStyle(Theme.secondary)
                .padding(.top, 4)

            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("$").foregroundStyle(Theme.codePrompt)
                Text(request.command).foregroundStyle(Theme.codeText)
            }
            .font(Theme.mono(12.5))
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.codeBackground, in: .rect(cornerRadius: 18))
            .padding(.top, 16)
            .textSelection(.enabled)

            Text(request.cwd)
                .font(Theme.mono(11.5))
                .foregroundStyle(Theme.tertiary)
                .padding(.horizontal, 4)
                .padding(.top, 8)

            Toggle(request.ruleLabel, isOn: $model.allowForSession)
                .font(.system(size: 15))
                .tint(Theme.accent)
                .padding(.horizontal, 14)
                .frame(height: 52)
                .background(Color.white, in: .rect(cornerRadius: 18))
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
        .sensoryFeedback(.warning, trigger: request.id)
    }
}

/// `session.ended { reason: taken_back }` (§8.4).
struct TakenBackSheet: View {
    let hostName: String
    let title: String
    var onReconnect: () -> Void
    var onClose: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 24))
                .foregroundStyle(Theme.accent)
                .frame(width: 56, height: 56)
                .background(Theme.accent.opacity(0.12), in: .circle)
                .padding(.bottom, 14)
            Text("\(hostName) took this session back")
                .font(.system(size: 19, weight: .semibold))
                .multilineTextAlignment(.center)
                .padding(.bottom, 6)
            Text("“\(title)” is controlled on the desktop now. Reconnect to take control again.")
                .font(.system(size: 14.5))
                .foregroundStyle(Theme.secondary)
                .multilineTextAlignment(.center)
                .padding(.bottom, 22)
            VStack(spacing: 10) {
                Button("Reconnect", action: onReconnect).buttonStyle(PrimaryButtonStyle())
                Button("Close", action: onClose).buttonStyle(SecondaryButtonStyle())
            }
        }
        .padding(.horizontal, 22)
        .padding(.top, 28)
    }
}
