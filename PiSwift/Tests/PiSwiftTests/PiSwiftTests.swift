import XCTest
import PiSwift

final class PiSwiftTests: XCTestCase {
    func testRunThrowsOnInvalidBaseURL() async {
        let client = PiSwiftClient(
            config: PiSwiftConfig(
                apiKey: "x",
                baseURL: "not a url",
                model: "gpt-4o-mini"
            )
        )

        do {
            _ = try await client.run(prompt: "hi")
            XCTFail("Expected error")
        } catch {
            // Any error is fine; we just want deterministic, offline coverage that the FFI is wired.
        }
    }

    func testRunTranscriptJSONThrowsOnInvalidBaseURL() async {
        let client = PiSwiftClient(
            config: PiSwiftConfig(
                apiKey: "x",
                baseURL: "not a url",
                model: "gpt-4o-mini"
            )
        )

        do {
            _ = try await client.runTranscriptJSON(prompt: "hi")
            XCTFail("Expected error")
        } catch {
            // Any error is fine; we just want deterministic, offline coverage that the FFI is wired.
        }
    }
}

