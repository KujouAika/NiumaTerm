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
        .sensoryFeedback(.warning, trigger: approval.description)
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
