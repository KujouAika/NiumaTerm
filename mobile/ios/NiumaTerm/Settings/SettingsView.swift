import SwiftUI

/// Settings (§8.1): paired hosts, notifications, terminal, agent, security, about.
struct SettingsView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss

    @AppStorage("notifyTurnFinished") private var notifyTurnFinished = true
    @AppStorage("notifyApproval") private var notifyApproval = true
    @AppStorage("notifyQuestion") private var notifyQuestion = true
    @AppStorage("notifyError") private var notifyError = true
    @AppStorage("terminalFontName") private var fontName = "JetBrains Mono"
    @AppStorage("terminalFontSize") private var fontSize = 11.0
    @AppStorage("transcriptMono") private var transcriptMono = true
    @AppStorage("faceIDLock") private var faceIDLock = false

    @State private var forgetting: Host?

    var body: some View {
        NavigationStack {
            Form {
                Section("Paired computers") {
                    ForEach(app.hosts) { host in
                        HStack(spacing: 12) {
                            Image(systemName: host.icon).foregroundStyle(Theme.ink2).frame(width: 24)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(host.name)
                                Text(host.statusText).font(.caption).foregroundStyle(Theme.secondary)
                            }
                            Spacer()
                            Button("Forget", role: .destructive) { forgetting = host }
                                .buttonStyle(.borderless)
                        }
                    }
                }
                Section {
                    Toggle("Turn finished", isOn: $notifyTurnFinished)
                    Toggle("Approval needed", isOn: $notifyApproval)
                    Toggle("Questions", isOn: $notifyQuestion)
                    Toggle("Errors", isOn: $notifyError)
                } header: {
                    Text("Notifications")
                } footer: {
                    Text("Sent by each computer while this phone is not viewing the session.")
                }
                Section("Terminal") {
                    Picker("Font", selection: $fontName) {
                        Text("JetBrains Mono").tag("JetBrains Mono")
                        Text("SF Mono").tag("SF Mono")
                    }
                    Stepper(value: $fontSize, in: 8...18, step: 1) {
                        LabeledContent("Size", value: "\(Int(fontSize)) pt")
                    }
                }
                Section("Agent") {
                    Toggle("Monospaced transcript", isOn: $transcriptMono)
                }
                Section("Security") {
                    Toggle("Require Face ID", isOn: $faceIDLock)
                }
                Section {
                    LabeledContent("Version", value: AppModel.appVersion)
                }
            }
            .tint(Theme.accent)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .confirmationDialog(
                "Forget \(forgetting?.name ?? "")?",
                isPresented: Binding(get: { forgetting != nil }, set: { if !$0 { forgetting = nil } }),
                titleVisibility: .visible,
                presenting: forgetting
            ) { host in
                Button("Forget \(host.name)", role: .destructive) { app.forget(host.id) }
            } message: { _ in
                Text("This phone will need to scan a new QR code to reach it again.")
            }
        }
    }
}
