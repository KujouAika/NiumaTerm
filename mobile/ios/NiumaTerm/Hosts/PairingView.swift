import SwiftUI
import VisionKit

/// "Add computer": QR scanner for `niumaterm://pair?...` links, typed code as fallback (§7.1).
struct PairingView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var showManual = false
    @State private var pairing = false

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
                ManualPairingForm { name in
                    app.addPairedHost(name: name)
                    dismiss()
                }
            }
        }
        .environment(\.colorScheme, .dark)
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
                Button("Enter code instead") { showManual = true }
                    .buttonStyle(SecondaryButtonStyle(dark: true))
                #if targetEnvironment(simulator)
                Button("Simulate scan") { pair("niumaterm://pair?code=DEMO") }
                    .font(.system(size: 14, weight: .medium))
                    .padding(.top, 12)
                #endif
            }
        }
        .foregroundStyle(Color.white)
        .padding(.horizontal, 24)
        .padding(.vertical, 28)
        .frame(maxWidth: .infinity)
        .glassRounded(36)
        .padding(8)
    }

    /// Real app: `MobileCore.pair(link_or_code:relay:)`.
    private func pair(_ link: String) {
        guard !pairing else { return }
        pairing = true
        Task {
            try? await Task.sleep(for: .seconds(1.2))
            app.addPairedHost(name: "Office PC")
            dismiss()
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
    var onPaired: (String) -> Void
    @State private var code = ""
    @State private var relay = ""
    @State private var accessKey = ""
    @State private var pairing = false

    var body: some View {
        Form {
            Section {
                TextField("Pairing code", text: $code)
                    .font(Theme.mono(17))
                    .textInputAutocapitalization(.characters)
                    .autocorrectionDisabled()
            } footer: {
                Text("Shown next to the QR code on the desktop.")
            }
            Section("Relay") {
                TextField("Relay URL", text: $relay)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                SecureField("Access key", text: $accessKey)
            }
            Section {
                Button {
                    pairing = true
                    Task {
                        try? await Task.sleep(for: .seconds(1))
                        onPaired("Office PC")
                    }
                } label: {
                    HStack {
                        Text("Pair")
                        if pairing { Spacer(); ProgressView() }
                    }
                }
                .disabled(code.isEmpty || relay.isEmpty || pairing)
            }
        }
        .navigationTitle("Enter Code")
        .navigationBarTitleDisplayMode(.inline)
    }
}
