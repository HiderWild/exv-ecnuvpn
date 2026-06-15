#include <iostream>
#include <string>

import exv.base.types;
import exv.base.errors;

int main() {
  bool ok = true;
  auto status = exv::base::Status::success_status();
  ok = ok && status.ok();

  auto failure =
      exv::base::Status::failure("config_invalid", "Config is invalid");
  ok = ok && !failure.ok();
  ok = ok && failure.code == "config_invalid";

  exv::base::ErrorInfo error;
  error.domain = exv::base::error_domains::Config;
  error.code = exv::base::error_codes::InvalidConfig;
  error.message = "Config is invalid";
  ok = ok && error.domain == "config";

  if (!ok) {
    std::cerr << "base_module_smoke_test failed\n";
    return 1;
  }

  std::cout << "base_module_smoke_test passed\n";
  return 0;
}
