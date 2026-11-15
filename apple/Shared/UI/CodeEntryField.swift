import SwiftUI
import WarpCore

/// "Send to code…" field: six digits with an auto-inserted dash. Accepts
/// pasted `482910`, `482-910` or a full `warpdrop://` link (whose direct
/// addresses are kept and passed on, so a scanned/pasted ticket connects
/// without discovery).
struct CodeEntryField: View {
    var placeholder: String
    /// Submit automatically once six digits are present.
    var autoSubmit = true
    var large = false
    var onSubmit: (String) -> Void

    @State private var text = ""
    @State private var link: String?
    @FocusState private var focused: Bool

    private var digits: String { text.filter(\.isNumber) }
    private var isComplete: Bool { digits.count == 6 }

    var body: some View {
        HStack(spacing: Theme.Space.xs) {
            TextField(placeholder, text: $text)
                .textFieldStyle(.plain)
                .font(large ? .system(size: 22, weight: .semibold).monospacedDigit() : Theme.Fonts.body.monospacedDigit())
                .tracking(large ? 22 * 0.04 : 0.5)
                .focused($focused)
                .autocorrectionDisabled()
                #if os(iOS)
                .keyboardType(.numbersAndPunctuation)
                .textInputAutocapitalization(.never)
                .submitLabel(.send)
                #endif
                .onSubmit(submit)
                .onChange(of: text) { _, new in sanitize(new) }
                .accessibilityHint(L("send_to_code_hint"))

            Button(action: submit) {
                Image(systemName: "arrow.right")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(isComplete ? Color.white : Theme.textTertiary)
                    .frame(width: 26, height: 26)
                    .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(isComplete ? Theme.accent : Theme.surfaceHover))
            }
            .buttonStyle(.plain)
            .disabled(!isComplete)
            .accessibilityLabel(L("send"))
        }
        .padding(.leading, Theme.Space.s)
        .padding(.trailing, 4)
        .frame(height: large ? 48 : 34)
        .cardSurface(fill: Theme.surface, border: focused ? Theme.accent : Theme.border, radius: Theme.Radius.control)
        .animation(Theme.Motion.spring, value: isComplete)
    }

    /// Keeps only digits (formatted `123-456`), or swallows a pasted link.
    private func sanitize(_ input: String) {
        let trimmed = input.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.lowercased().hasPrefix("warpdrop:") {
            // Re-enters with the formatted code, which then auto-submits.
            if let code = normalizeCode(input: trimmed) {
                link = trimmed
                setText(formatCode(code: code))
            } else {
                setText("")
            }
            return
        }
        let d = String(input.filter(\.isNumber).prefix(6))
        let formatted = d.count > 3 ? "\(d.prefix(3))-\(d.dropFirst(3))" : d
        if formatted != input {
            setText(formatted)
            return
        }
        guard d.count == 6 else {
            link = nil
            lastSubmitted = nil
            return
        }
        if autoSubmit, lastSubmitted != d {
            submit()
        }
    }

    @State private var lastSubmitted: String?

    private func setText(_ value: String) {
        if text != value { text = value }
    }

    private func submit() {
        guard isComplete else { return }
        let target = link ?? digits
        lastSubmitted = digits
        onSubmit(target)
        // Ready for the next code.
        DispatchQueue.main.async {
            text = ""
            link = nil
            lastSubmitted = nil
            focused = false
        }
    }
}
