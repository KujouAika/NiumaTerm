import SwiftUI
import VisionKit
import os

private let log = Logger(subsystem: Bundle.main.bundleIdentifier ?? "NiumaTerm", category: "pairing")

/// "Add computer": QR scanner for `niumaterm://pair?...` links, typed code as fallback (§7.1).
struct PairingView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss

    /// A link the app was opened with, paired at once.
    var initialLink: String?

    @State private var showManual = false
    @State private var pairing = false
    @State private var error: String?

    private var scannerAvailable: Bool {
        DataScannerViewController.isSupported && DataScannerViewController.isAvailable
    }

    var body: some View {
        NavigationStack {
            ZStack {
                Theme.codeBackground.ignoresSafeArea()

                if scannerAvailable {
                    QRScannerView { link in pair(link) }
                        .ignoresSafeArea()
                } else {
                    Text("Camera unavailable")
                        .font(Theme.mono(12))
                        .foregroundStyle(Color.white.opacity(0.4))
                        .offset(y: -80)
                }

                ViewfinderCorners()
                    .stroke(Color.white, style: StrokeStyle(lineWidth: 4, lineCap: .round))
                    .frame(width: 240, height: 240)
                    .offset(y: -80)

                VStack {
                    Spacer()

                    panel
                }
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
            }
            .navigationDestination(isPresented: $showManual) {
                ManualPairingForm { dismiss() }
            }
        }
        .environment(\.colorScheme, .dark)
        .task {
            log.info("pairing screen shown, link: \(initialLink != nil)")

            if let initialLink { pair(initialLink) }
        }
    }

    private var panel: some View {
        VStack(spacing: 0) {
            if pairing {
                ProgressView().controlSize(.large).padding(.bottom, 14)
                Text("Pairing…").font(.system(size: 19, weight: .semibold))
            } else {
                Text("Add a computer")
                    .font(.system(size: 21, weight: .semibold))
                    .padding(.bottom, 8)

                Text("On your desktop, open Settings › Remote › This computer and scan the QR code. The code works once, for 5 minutes.")
                    .font(.system(size: 14.5))
                    .foregroundStyle(Color.white.opacity(0.72))
                    .multilineTextAlignment(.center)
                    .padding(.bottom, 22)

                if let error {
                    Text(error)
                        .font(.system(size: 14))
                        .foregroundStyle(Theme.codePrompt)
                        .multilineTextAlignment(.center)
                        .padding(.bottom, 16)
                }

                Button("Enter code instead") { showManual = true }
                    .buttonStyle(SecondaryButtonStyle(dark: true))

                // The desktop's "Copy link" puts the same link on the
                // clipboard, which reaches a simulator or an iPad without a
                // camera pointed at the screen.
                Button("Paste pairing link", systemImage: "doc.on.clipboard") {
                    if let link = UIPasteboard.general.string, !link.isEmpty {
                        pair(link)
                    } else {
                        error = tr("The clipboard holds no pairing link. Use Copy link on the computer first.")
                    }
                }
                .font(.system(size: 14, weight: .medium))
                .padding(.top, 12)
            }
        }
        .foregroundStyle(Color.white)
        .padding(.horizontal, 24)
        .padding(.vertical, 28)
        .frame(maxWidth: .infinity)
        .glassRounded(36)
        .padding(8)
    }

    private func pair(_ link: String) {
        log.info("pairing requested, busy: \(pairing)")

        guard !pairing else { return }

        pairing = true
        error = nil

        Task {
            do {
                _ = try await app.pair(link)

                dismiss()
            } catch {
                log.error("pairing failed: \(error.displayText, privacy: .public)")

                self.error = error.displayText
                pairing = false
            }
        }
    }
}

struct ViewfinderCorners: Shape {
    func path(in r: CGRect) -> Path {
        var p = Path()

        let length: CGFloat = 44, radius: CGFloat = 22

        func corner(_ o: CGPoint, _ sx: CGFloat, _ sy: CGFloat) {
            p.move(to: CGPoint(x: o.x, y: o.y + sy * length))
            p.addLine(to: CGPoint(x: o.x, y: o.y + sy * radius))
            p.addQuadCurve(to: CGPoint(x: o.x + sx * radius, y: o.y), control: o)
            p.addLine(to: CGPoint(x: o.x + sx * length, y: o.y))
        }

        corner(CGPoint(x: r.minX, y: r.minY), 1, 1)
        corner(CGPoint(x: r.maxX, y: r.minY), -1, 1)
        corner(CGPoint(x: r.minX, y: r.maxY), 1, -1)
        corner(CGPoint(x: r.maxX, y: r.maxY), -1, -1)

        return p
    }
}

struct QRScannerView: UIViewControllerRepresentable {
    var onLink: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let vc = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])],
                                           qualityLevel: .balanced,
                                           isHighlightingEnabled: false)

        vc.delegate = context.coordinator

        return vc
    }

    func updateUIViewController(_ vc: DataScannerViewController, context: Context) {
        if !vc.isScanning { try? vc.startScanning() }
    }

    func makeCoordinator() -> Coordinator { Coordinator(onLink: onLink) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let onLink: (String) -> Void
        private var done = false

        init(onLink: @escaping (String) -> Void) { self.onLink = onLink }

        func dataScanner(_ dataScanner: DataScannerViewController, didAdd addedItems: [RecognizedItem], allItems: [RecognizedItem]) {
            guard !done else { return }

            for item in addedItems {
                if case .barcode(let code) = item,
                   let value = code.payloadStringValue,
                   value.hasPrefix("niumaterm://pair") {
                    done = true

                    onLink(value)

                    return
                }
            }
        }
    }
}

struct ManualPairingForm: View {
    @Environment(AppModel.self) private var app
    var onPaired: () -> Void
    @State private var code = ""
    @State private var relay = ""
    @State private var accessKey = ""
    @State private var pairing = false
    @State private var error: String?

    var body: some View {
        Form {
            Section {
                TextField("Pairing code or link", text: $code)
                    .font(Theme.mono(17))
                    .textInputAutocapitalization(.characters)
                    .autocorrectionDisabled()
            } footer: {
                Text("Shown next to the QR code on the desktop. A copied pairing link works here too.")
            }

            if !isLink {
                Section("Relay") {
                    TextField("Relay URL", text: $relay)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()

                    SecureField("Access key", text: $accessKey)
                }
            }

            if let error {
                Section {
                    Text(error).foregroundStyle(Theme.attention)
                }
            }

            Section {
                Button(action: pair) {
                    HStack {
                        Text("Pair")

                        if pairing { Spacer(); ProgressView() }
                    }
                }
                .disabled(code.isEmpty || (!isLink && (relay.isEmpty || accessKey.isEmpty)) || pairing)
            }
        }
        .navigationTitle(tr("Enter Code"))
        .navigationBarTitleDisplayMode(.inline)
    }

    private var isLink: Bool {
        code.trimmingCharacters(in: .whitespaces).lowercased().hasPrefix("niumaterm:")
    }

    private func pair() {
        pairing = true
        error = nil

        Task {
            do {
                if isLink {
                    _ = try await app.pair(code)
                } else {
                    _ = try await app.pair(code, relayURL: relay, accessKey: accessKey)
                }

                onPaired()
            } catch {
                self.error = error.displayText
                pairing = false
            }
        }
    }
}
