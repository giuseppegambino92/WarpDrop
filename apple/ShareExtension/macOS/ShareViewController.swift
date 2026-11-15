import AppKit
import SwiftUI

/// macOS Share menu → "WarpDrop".
final class ShareViewController: NSViewController {
    private let model = ShareModel()

    override var nibName: NSNib.Name? { nil }

    override func loadView() {
        let hosting = NSHostingView(rootView: ShareView().environment(model))
        hosting.frame = NSRect(x: 0, y: 0, width: 400, height: 460)
        view = hosting
        preferredContentSize = NSSize(width: 400, height: 460)
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? []).flatMap { $0.attachments ?? [] }
        model.start(providers: providers) { [weak self] completed in
            guard let context = self?.extensionContext else { return }
            if completed {
                context.completeRequest(returningItems: nil)
            } else {
                context.cancelRequest(withError: NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
            }
        }
    }
}
