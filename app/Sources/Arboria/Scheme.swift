import Foundation
import WebKit

/// `arboria://app/…` and `arboria://fonts/…`: only files inside the signed app bundle,
/// never the repository or any writable directory (ADR 0004, what you see is what you sign).
final class BundleScheme: NSObject, WKURLSchemeHandler {
    let root = Bundle.main.resourceURL!

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        guard let url = task.request.url, let host = url.host, ["app", "fonts"].contains(host) else {
            task.didFailWithError(URLError(.badURL)); return
        }
        let base = root.appendingPathComponent(host == "fonts" ? "fonts" : "web").standardizedFileURL
        let file = base.appendingPathComponent(url.path).standardizedFileURL
        guard file.path.hasPrefix(base.path + "/"), let data = try? Data(contentsOf: file) else {
            task.didFailWithError(URLError(.fileDoesNotExist)); return
        }
        let types = ["html": "text/html; charset=utf-8", "css": "text/css", "js": "text/javascript", "woff2": "font/woff2"]
        // The notebook page is http://127.0.0.1 and loads our fonts cross-origin (P0 spike 3).
        let headers = ["Content-Type": types[file.pathExtension] ?? "application/octet-stream",
                       "Access-Control-Allow-Origin": "*", "Content-Length": "\(data.count)"]
        task.didReceive(HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: headers)!)
        task.didReceive(data)
        task.didFinish()
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}
}
