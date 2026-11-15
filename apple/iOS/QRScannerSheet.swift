import AVFoundation
import SwiftUI
import UIKit

/// Scans another device's WarpDrop QR (`warpdrop://c/<code>?pk=…&a=…`).
struct QRScannerSheet: View {
    var onScan: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var authorization = AVCaptureDevice.authorizationStatus(for: .video)

    var body: some View {
        NavigationStack {
            ZStack {
                Color.black.ignoresSafeArea()
                switch authorization {
                case .authorized:
                    QRScannerRepresentable(onScan: onScan)
                        .ignoresSafeArea()
                    RoundedRectangle(cornerRadius: Theme.Radius.card, style: .continuous)
                        .strokeBorder(Color.white.opacity(0.9), lineWidth: 1.5)
                        .frame(width: 240, height: 240)
                        .accessibilityHidden(true)
                case .notDetermined:
                    ProgressView().tint(.white)
                default:
                    VStack(spacing: Theme.Space.m) {
                        Text(L("camera_denied"))
                            .font(Theme.Fonts.body)
                            .foregroundStyle(.white)
                            .multilineTextAlignment(.center)
                        Button(L("settings")) {
                            if let url = URL(string: UIApplication.openSettingsURLString) {
                                UIApplication.shared.open(url)
                            }
                        }
                        .buttonStyle(QuietButtonStyle())
                    }
                    .padding(Theme.Space.xxl)
                }
            }
            .navigationTitle(L("scan_qr"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("cancel")) { dismiss() }
                }
            }
        }
        .task {
            if authorization == .notDetermined {
                let granted = await AVCaptureDevice.requestAccess(for: .video)
                authorization = granted ? .authorized : .denied
            }
        }
    }
}

private struct QRScannerRepresentable: UIViewControllerRepresentable {
    var onScan: (String) -> Void

    func makeUIViewController(context: Context) -> QRScannerViewController {
        let vc = QRScannerViewController()
        vc.onScan = onScan
        return vc
    }

    func updateUIViewController(_ vc: QRScannerViewController, context: Context) {
        vc.onScan = onScan
    }
}

/// AVCaptureSession + AVCaptureMetadataOutput (QR only).
final class QRScannerViewController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onScan: ((String) -> Void)?
    private let session = SessionBox()
    private var preview: AVCaptureVideoPreviewLayer?
    private var delivered = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        configure()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview?.frame = view.bounds
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        session.start()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        session.stop()
    }

    private func configure() {
        let capture = session.session
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              capture.canAddInput(input)
        else { return }
        capture.beginConfiguration()
        capture.addInput(input)
        let output = AVCaptureMetadataOutput()
        if capture.canAddOutput(output) {
            capture.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            if output.availableMetadataObjectTypes.contains(.qr) {
                output.metadataObjectTypes = [.qr]
            }
        }
        capture.commitConfiguration()
        let layer = AVCaptureVideoPreviewLayer(session: capture)
        layer.videoGravity = .resizeAspectFill
        layer.frame = view.bounds
        view.layer.addSublayer(layer)
        preview = layer
    }

    nonisolated func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
        let payload = metadataObjects
            .compactMap { ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }
            .first { $0.lowercased().hasPrefix("warpdrop:") || $0.filter(\.isNumber).count == 6 }
        guard let payload else { return }
        // Delegate queue is `.main`.
        MainActor.assumeIsolated { self.deliver(payload) }
    }

    private func deliver(_ payload: String) {
        guard !delivered else { return }
        delivered = true
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        session.stop()
        onScan?(payload)
    }
}

/// `AVCaptureSession` start/stop block; run them off the main thread.
private final class SessionBox: @unchecked Sendable {
    // AVCaptureSession is thread-safe for start/stop; all configuration
    // happens on the main thread before the first start.
    let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "app.warpdrop.camera")

    func start() {
        queue.async { [self] in
            if !session.isRunning { session.startRunning() }
        }
    }

    func stop() {
        queue.async { [self] in
            if session.isRunning { session.stopRunning() }
        }
    }
}
