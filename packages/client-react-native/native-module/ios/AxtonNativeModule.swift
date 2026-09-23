import ExpoModulesCore
import Foundation

private class AxtonNativeException: Exception {
  private let message: String

  init(_ message: String) {
    self.message = message
    super.init()
  }

  override var reason: String {
    message
  }
}

public final class AxtonNativeModule: Module {
  private let carrierQueue = DispatchQueue(label: "dev.axton.native.carrier")

  public func definition() -> ModuleDefinition {
    Name("AxtonNative")

    AsyncFunction("clientCall") { (request: String) async throws -> String in
      try await withCheckedThrowingContinuation { continuation in
        self.carrierQueue.async {
          continuation.resume(with: Result { try self.callCarrier(request) })
        }
      }
    }

    AsyncFunction("databasePath") { (name: String) throws -> String in
      do {
        return try MobileDatabasePath.resolve(name: name).path
      } catch MobileDatabasePath.Error.invalidBasename {
        throw AxtonNativeException("database name must be a basename")
      } catch {
        throw AxtonNativeException("could not create Application Support directory: \(error)")
      }
    }
  }

  private func callCarrier(_ request: String) throws -> String {
    guard let output = request.withCString({ axton_mobile_call($0) }) else {
      throw AxtonNativeException("AXTON native carrier returned null")
    }
    defer { axton_mobile_free(output) }

    let data = Data(String(cString: output).utf8)
    let envelope: [String: Any]
    do {
      guard let decoded = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw AxtonNativeException("AXTON native carrier returned a non-object envelope")
      }
      envelope = decoded
    } catch let error as AxtonNativeException {
      throw error
    } catch {
      throw AxtonNativeException("AXTON native carrier returned invalid JSON: \(error)")
    }
    guard envelope["ok"] as? Bool == true else {
      throw AxtonNativeException(envelope["error"] as? String ?? "AXTON native call failed")
    }
    guard let result = envelope["result"] else {
      throw AxtonNativeException("AXTON native carrier omitted result")
    }
    do {
      let resultData = try JSONSerialization.data(withJSONObject: result, options: [.fragmentsAllowed])
      guard let text = String(data: resultData, encoding: .utf8) else {
        throw AxtonNativeException("AXTON native result is not UTF-8")
      }
      return text
    } catch let error as AxtonNativeException {
      throw error
    } catch {
      throw AxtonNativeException("AXTON native result could not be serialized: \(error)")
    }
  }
}
