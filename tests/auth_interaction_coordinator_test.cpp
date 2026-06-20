#include "core/app_api/auth_interaction_coordinator.hpp"

#include <atomic>
#include <chrono>
#include <iostream>
#include <stop_token>
#include <string>
#include <thread>

namespace {

bool expect(bool condition, const char *message) {
  if (condition) return true;
  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

bool round_trip_unblocks_handler_with_response_value() {
  using ecnuvpn::app_api::AuthInteractionCoordinator;
  using ecnuvpn::vpn_engine::protocol::AuthInteractionRequest;
  using ecnuvpn::vpn_engine::protocol::AuthInteractionResponse;

  bool ok = true;
  AuthInteractionCoordinator coordinator;

  AuthInteractionRequest req;
  req.id = "auth-group-continuation";
  req.kind = "group";
  req.label = "Choose a VPN group";
  req.input_type = "select";
  req.options = {"students", "staff"};

  std::atomic<bool> handler_returned{false};
  AuthInteractionResponse out;
  std::thread handler([&] {
    out = coordinator.handle(req);
    handler_returned = true;
  });

  // Spin briefly until the prompt is published. The coordinator must expose
  // it to the RPC layer before handle() blocks.
  for (int i = 0; i < 200 && !coordinator.pending(); ++i) {
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
  ok = expect(coordinator.pending().has_value(),
              "pending() must surface the prompt while handle() blocks") && ok;
  if (auto pending = coordinator.pending(); pending) {
    ok = expect(pending->id == req.id,
                "pending() must echo the request id") && ok;
    ok = expect(pending->kind == req.kind,
                "pending() must echo the request kind") && ok;
    ok = expect(pending->options.size() == 2,
                "pending() must echo the request options") && ok;
  }

  ok = expect(coordinator.respond("auth-group-continuation", "students"),
              "respond() with the matching id must succeed") && ok;
  handler.join();
  ok = expect(handler_returned.load(),
              "handler must return after respond()") && ok;
  ok = expect(out.ok, "handle() must return ok=true after respond()") && ok;
  ok = expect(out.value == "students",
              "handle() must return the value supplied by respond()") && ok;
  ok = expect(!coordinator.pending().has_value(),
              "pending() must clear after handle() returns") && ok;
  return ok;
}

bool respond_with_mismatched_id_does_not_unblock_handler() {
  using ecnuvpn::app_api::AuthInteractionCoordinator;
  using ecnuvpn::vpn_engine::protocol::AuthInteractionRequest;
  using ecnuvpn::vpn_engine::protocol::AuthInteractionResponse;

  bool ok = true;
  AuthInteractionCoordinator coordinator;

  AuthInteractionRequest req;
  req.id = "auth-challenge-continuation";
  req.kind = "challenge";

  std::atomic<bool> handler_returned{false};
  AuthInteractionResponse out;
  std::thread handler([&] {
    out = coordinator.handle(req);
    handler_returned = true;
  });

  for (int i = 0; i < 200 && !coordinator.pending(); ++i) {
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
  ok = expect(!coordinator.respond("wrong-id", "ignored"),
              "respond() with a stale id must return false") && ok;
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  ok = expect(!handler_returned.load(),
              "stale respond() must NOT unblock handle()") && ok;

  // Clean up: cancel so the handler thread exits.
  coordinator.cancel();
  handler.join();
  ok = expect(!out.ok,
              "cancelled handle() must return ok=false") && ok;
  return ok;
}

bool stop_token_cancels_handler() {
  using ecnuvpn::app_api::AuthInteractionCoordinator;
  using ecnuvpn::vpn_engine::protocol::AuthInteractionRequest;
  using ecnuvpn::vpn_engine::protocol::AuthInteractionResponse;

  bool ok = true;
  AuthInteractionCoordinator coordinator;
  std::stop_source stop;

  AuthInteractionRequest req;
  req.id = "auth-group-continuation";

  std::atomic<bool> handler_returned{false};
  AuthInteractionResponse out;
  std::thread handler([&] {
    out = coordinator.handle(req, stop.get_token());
    handler_returned = true;
  });

  for (int i = 0; i < 200 && !coordinator.pending(); ++i) {
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
  stop.request_stop();
  handler.join();
  ok = expect(handler_returned.load(),
              "stop_token must unblock handle()") && ok;
  ok = expect(!out.ok,
              "stop_token cancellation must yield ok=false") && ok;
  return ok;
}

bool global_slot_is_round_trippable() {
  using ecnuvpn::app_api::AuthInteractionCoordinator;
  using ecnuvpn::app_api::get_active_connect_auth_coordinator;
  using ecnuvpn::app_api::set_active_connect_auth_coordinator;

  bool ok = true;
  ok = expect(!get_active_connect_auth_coordinator(),
              "global slot must start empty") && ok;

  auto coordinator = std::make_shared<AuthInteractionCoordinator>();
  set_active_connect_auth_coordinator(coordinator);
  auto fetched = get_active_connect_auth_coordinator();
  ok = expect(fetched.get() == coordinator.get(),
              "global slot must return the published coordinator") && ok;

  set_active_connect_auth_coordinator(nullptr);
  ok = expect(!get_active_connect_auth_coordinator(),
              "global slot must clear after publishing nullptr") && ok;
  return ok;
}

bool stale_global_slot_clear_does_not_cancel_newer_coordinator() {
  using ecnuvpn::app_api::AuthInteractionCoordinator;
  using ecnuvpn::app_api::clear_active_connect_auth_coordinator_if_current;
  using ecnuvpn::app_api::get_active_connect_auth_coordinator;
  using ecnuvpn::app_api::set_active_connect_auth_coordinator;
  using ecnuvpn::vpn_engine::protocol::AuthInteractionRequest;
  using ecnuvpn::vpn_engine::protocol::AuthInteractionResponse;

  bool ok = true;
  auto stale = std::make_shared<AuthInteractionCoordinator>();
  auto current = std::make_shared<AuthInteractionCoordinator>();
  set_active_connect_auth_coordinator(stale);
  set_active_connect_auth_coordinator(current);

  AuthInteractionRequest req;
  req.id = "new-connect-group";
  req.kind = "group";

  std::atomic<bool> handler_returned{false};
  AuthInteractionResponse out;
  std::thread handler([&] {
    out = current->handle(req);
    handler_returned = true;
  });

  for (int i = 0; i < 200 && !current->pending(); ++i) {
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }

  const bool cleared =
      clear_active_connect_auth_coordinator_if_current(stale);

  ok = expect(!cleared,
              "stale coordinator clear must report that it did not own the "
              "active global slot") &&
       ok;
  ok = expect(get_active_connect_auth_coordinator().get() == current.get(),
              "stale coordinator clear must preserve the newer global slot") &&
       ok;
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  ok = expect(!handler_returned.load(),
              "stale coordinator clear must not cancel the newer pending "
              "interaction") &&
       ok;

  if (!handler_returned.load()) {
    ok = expect(current->respond("new-connect-group", "students"),
                "newer coordinator must still accept the matching response") &&
         ok;
  } else {
    current->cancel();
  }
  handler.join();
  ok = expect(out.ok,
              "newer coordinator handle() must complete from respond(), not "
              "from stale cleanup cancellation") &&
       ok;
  ok = expect(out.value == "students",
              "newer coordinator must preserve the response value") &&
       ok;

  clear_active_connect_auth_coordinator_if_current(current);
  ok = expect(!get_active_connect_auth_coordinator(),
              "current coordinator clear must empty the global slot") && ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = round_trip_unblocks_handler_with_response_value() && ok;
  ok = respond_with_mismatched_id_does_not_unblock_handler() && ok;
  ok = stop_token_cancels_handler() && ok;
  ok = global_slot_is_round_trippable() && ok;
  ok = stale_global_slot_clear_does_not_cancel_newer_coordinator() && ok;
  if (ok) {
    std::cout << "auth_interaction_coordinator_test: all assertions passed\n";
  } else {
    std::cerr << "auth_interaction_coordinator_test: FAILED\n";
  }
  return ok ? 0 : 1;
}
