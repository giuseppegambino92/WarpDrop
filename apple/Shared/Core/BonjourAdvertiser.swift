import Foundation
import dnssd
import os
import WarpCore

/// Publishes this device's QUIC endpoint with the C DNS-SD API.
///
/// `kDNSServiceFlagsIncludeP2P` makes the service visible over AWDL (Wi-Fi
/// peer-to-peer) as well, which NWListener-free registration cannot do. The
/// advertised port is the Rust QUIC port — we never open a socket here.
@MainActor
final class BonjourAdvertiser {
    private var service: DNSServiceRef?
    private var current: ServiceAdvert?
    private let queue = DispatchQueue(label: "app.warpdrop.bonjour.advertise")
    private static let log = Logger(subsystem: "app.warpdrop", category: "bonjour")

    func start(_ advert: ServiceAdvert) {
        if let current, current.port == advert.port, current.instanceName == advert.instanceName, service != nil {
            if current.txt != advert.txt { updateTXT(advert) }
            return
        }
        stop()
        let txt = Self.encodeTXT(advert.txt)
        var ref: DNSServiceRef?
        let flags = DNSServiceFlags(kDNSServiceFlagsIncludeP2P)
        let status: DNSServiceErrorType = txt.withUnsafeBytes { raw in
            DNSServiceRegister(
                &ref,
                flags,
                0, // kDNSServiceInterfaceIndexAny (+ P2P through the flag)
                advert.instanceName,
                advert.serviceType,
                nil,
                nil,
                advert.port.bigEndian,
                UInt16(raw.count),
                raw.baseAddress,
                bonjourRegisterReply,
                nil
            )
        }
        guard status == DNSServiceErrorType(kDNSServiceErr_NoError), let ref else {
            Self.log.error("DNSServiceRegister failed: \(status)")
            return
        }
        DNSServiceSetDispatchQueue(ref, queue)
        service = ref
        current = advert
    }

    /// Updates the primary TXT record in place (e.g. after a rename).
    private func updateTXT(_ advert: ServiceAdvert) {
        guard let service else { return }
        let txt = Self.encodeTXT(advert.txt)
        let status = txt.withUnsafeBytes { raw in
            DNSServiceUpdateRecord(service, nil, 0, UInt16(raw.count), raw.baseAddress, 0)
        }
        if status == DNSServiceErrorType(kDNSServiceErr_NoError) {
            current = advert
        } else {
            Self.log.error("DNSServiceUpdateRecord failed: \(status)")
        }
    }

    func stop() {
        if let service {
            DNSServiceRefDeallocate(service)
        }
        service = nil
        current = nil
    }

    /// RFC 6763 TXT encoding: length-prefixed `key=value` strings.
    static func encodeTXT(_ txt: [String: String]) -> Data {
        var out = Data()
        for key in txt.keys.sorted() {
            var entry = Array("\(key)=\(txt[key] ?? "")".utf8)
            if entry.count > 255 { entry = Array(entry.prefix(255)) }
            out.append(UInt8(entry.count))
            out.append(contentsOf: entry)
        }
        if out.isEmpty { out.append(0) }
        return out
    }
}

/// DNS-SD reply, called on the advertiser's private queue. Declared at file
/// scope so it is not MainActor-isolated (a closure literal inside the
/// MainActor class would be, and would trap the runtime isolation check).
private let bonjourRegisterReply: DNSServiceRegisterReply = { _, _, error, name, _, _, _ in
    let log = Logger(subsystem: "app.warpdrop", category: "bonjour")
    if error != DNSServiceErrorType(kDNSServiceErr_NoError) {
        log.error("DNSServiceRegister callback error \(error)")
    } else if let name {
        log.debug("registered \(String(cString: name), privacy: .public)")
    }
}
