import SwiftUI

struct TerminalSessionView: View {
    @Bindable var model: TerminalSessionModel
    let workspaceName: String

    @FocusState private var inputFocused: Bool
    @AppStorage("terminalFontSize") private var fontSize = 11.0
    @AppStorage("terminalFontName") private var fontName = "JetBrains Mono"
    @State private var pinchBase: Double?

    private var font: Font { Theme.terminalFont(fontSize, name: fontName) }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(model.lines.enumerated()), id: \.offset) { _, line in
                        Text(line)
                            .lineLimit(1)
                            .fixedSize(horizontal: true, vertical: false)
                    }
                    HStack(spacing: 0) {
                        Text(model.prompt + model.input)
                            .lineLimit(1)
                            .fixedSize(horizontal: true, vertical: false)
                        if model.ctrl { Text("^").foregroundStyle(Theme.accent) }
                        Cursor(width: fontSize * 0.6, height: fontSize * 1.3)
                    }
                    .id("cursor")
                }
                .font(font)
                .monospacedDigit()
                .foregroundStyle(Theme.ink)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .defaultScrollAnchor(.bottom)
            .scrollDismissesKeyboard(.never)
            .onChange(of: model.lines.count) { proxy.scrollTo("cursor", anchor: .bottom) }
            .onChange(of: inputFocused) { proxy.scrollTo("cursor", anchor: .bottom) }
        }
        .contentShape(.rect)
        .onTapGesture { inputFocused = true }
        .simultaneousGesture(
            MagnifyGesture()
                .onChanged { value in
                    let base = pinchBase ?? fontSize
                    pinchBase = base
                    fontSize = min(18, max(8, (base * value.magnification).rounded()))
                }
                .onEnded { _ in pinchBase = nil }
        )
        .background(Theme.terminalBackground)
        .onGeometryChange(for: CGSize.self) { $0.size } action: { size in
            // Real app: the phone's grid claims the PTY size while it has control.
            model.cols = max(20, Int((size.width - 24) / (fontSize * 0.6)))
            model.rows = max(8, Int(size.height / (fontSize * 1.45)))
        }
        .overlay(alignment: .bottomLeading) {
            // Hidden input. Real app: a UIView adopting UITextInput so IME marked text draws at the cursor.
            TextField("", text: $model.input)
                .focused($inputFocused)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.return)
                .onSubmit {
                    model.submit()
                    inputFocused = true
                }
                .onChange(of: model.input) { old, new in model.inputChanged(from: old, to: new) }
                .frame(width: 1, height: 1)
                .opacity(0.01)
                .accessibilityHidden(true)
        }
        .safeAreaInset(edge: .bottom) {
            AccessoryBar(model: model, keyboardShown: inputFocused) {
                inputFocused.toggle()
            }
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(model.session.title).font(.system(size: 16, weight: .semibold))
                    Text("\(workspaceName) · \(model.cols)×\(model.rows)")
                        .font(Theme.mono(11.5))
                        .foregroundStyle(Theme.secondary)
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("Copy all", systemImage: "doc.on.doc") {
                        UIPasteboard.general.string = model.lines.map { String($0.characters) }.joined(separator: "\n")
                    }
                    Button("Clear", systemImage: "eraser") { model.lines = [] }
                    Toggle("Keep desktop width", isOn: .constant(false)).disabled(true)
                } label: {
                    Image(systemName: "ellipsis")
                }
            }
        }
    }
}

private struct Cursor: View {
    let width: CGFloat
    let height: CGFloat
    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.55)) { context in
            let on = Int(context.date.timeIntervalSinceReferenceDate / 0.55) % 2 == 0
            Rectangle()
                .fill(Theme.ink)
                .frame(width: width, height: height)
                .opacity(on ? 1 : 0)
        }
    }
}

/// Glass key row above the keyboard (§8.2).
struct AccessoryBar: View {
    @Bindable var model: TerminalSessionModel
    let keyboardShown: Bool
    var toggleKeyboard: () -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(AccessoryKey.defaultLayout) { key in
                    let on = (key.action == .ctrl && model.ctrl) || (key.action == .alt && model.alt)
                    Button { model.press(key) } label: {
                        Text(key.label)
                            .font(Theme.mono(14, weight: .medium))
                            .foregroundStyle(on ? Color.white : Theme.ink)
                            .padding(.horizontal, 10)
                            .frame(minWidth: 40, minHeight: 40)
                            .background(on ? Theme.accent : Color.clear, in: .capsule)
                            .glassCapsule(interactive: true)
                    }
                    .buttonStyle(.plain)
                }
                Button(action: toggleKeyboard) {
                    Image(systemName: keyboardShown ? "keyboard.chevron.compact.down" : "keyboard")
                        .font(.system(size: 15))
                        .foregroundStyle(Theme.ink)
                        .frame(width: 44, height: 40)
                        .glassCapsule(interactive: true)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(keyboardShown ? "Hide keyboard" : "Show keyboard")
            }
            .padding(.horizontal, 8)
        }
        .padding(.vertical, 6)
    }
}
