#pragma once

#include <memory>
#include <string>

namespace exv::setup {

// True if current process token is elevated.
bool IsProcessElevated();

// Canonical string SID for the current process token user, or empty on failure.
std::wstring CurrentProcessUserSid();

// True if directory can be created/written by current user.
bool CanWriteDirectory(const std::wstring &directory);

// S2（2026-10-10 单次 UAC 提权会话）：客户端会话接口。首个提权步骤前一次
// runas 启动 worker，会话内多条命令（停服/清扫/起服）复用同一进程，安装结束
// Shutdown（OP quit）收尾。会话中途死亡（空闲超时/崩溃）时，下一条命令按需
// 重建——重建即再弹一次 UAC，launch_count() 如实累计（真机单 UAC 人工验收
// 归 W5）。
// 安全面说明（一行，复审 MINOR-4）：会话制延长提权 worker 存活窗，非提权
// GUI 可对其下达命令的窗口随之变长；令牌经命令行传递为既有形态，可接受。
class ElevatedSession {
 public:
  virtual ~ElevatedSession() = default;

  // 执行一条提权命令（stop_service/start_service/sweep_pfro）。失败时置
  // error 并返回 false；传输级失败（worker 死亡/超时）内部回收重建并重试
  // 一次，命令级 ERR 回复不重试（如实上抛）。
  virtual bool Call(const char *op, std::wstring *error = nullptr) = 0;

  // S5（2026-10-10 换盘服务迁移）：带参命令形态——参数（migrate_service 的
  // 新 ImagePath，可含非 ASCII/空格）经 UTF-8 无损编解码随 OP 行传递，取行尾
  // 全段（不作空白切分）；窄字符直转会破坏路径内容，禁止。语义与无参形态
  // 一致（传输级失败回收重建重试一次，命令级 ERR 如实上抛）。
  virtual bool Call(const char *op, const std::wstring &arg,
                    std::wstring *error = nullptr) = 0;

  // 会话收尾：OP quit + 限时等待 + 句柄回收；幂等，析构兜底调用。
  virtual void Shutdown() = 0;

  // 注入缝观测：累计 runas 启动次数（= UAC 提示次数）。
  virtual int launch_count() const = 0;
};

// 创建生产会话（runas worker）。pfro_sweep_dir 经宽字符命令行随 worker 携带
// （不经窄字符管道协议转码），仅 sweep_pfro 命令消费；纯服务会话可传空。
std::unique_ptr<ElevatedSession> CreateElevationSession(
    const std::wstring &pfro_sweep_dir = L"");

// S2：PFRO 撤销并入同一提权会话（原第二套独立 -pfro 管道实现已移除，
// sweep_pfro OP 复用同一 worker）。fail-closed 判定留在 install_engine 调用
// 侧：撤销不了就拒装，语义不变。
bool RunElevatedPfroSweep(ElevatedSession &session, std::wstring *error = nullptr);

// Stop an installed engine service while preserving its SCM registration.
// Uses the current token when elevated and otherwise the shared session worker.
bool EnsureServiceStopped(ElevatedSession &session);

// 恢复已有服务并确认 Running；非提升进程复用同一会话 worker。
bool EnsureServiceRunning(ElevatedSession &session, std::wstring *error = nullptr);

// S5（2026-10-10 换盘服务迁移）：迁移 exv-engine 服务 ImagePath 可执行段至
// new_engine_path（尾参保真，见 service_control.hpp）。ChangeServiceConfigW 需
// SERVICE_CHANGE_CONFIG——非提升安装 GUI 经同一会话 worker 的 migrate_service
// OP 承载；错误为 ASCII Win32 诊断，新旧路径的中文业务文案由调用侧拼接。
bool EnsureServiceImagePathMigrated(ElevatedSession &session,
                                    const std::wstring &new_engine_path,
                                    std::wstring *error = nullptr);

// Generate a simple one-time token (hex of random bytes).
std::wstring GenerateElevationToken();

// Default pipe name for this parent pid.
std::wstring DefaultElevationPipeName();

// Run elevated worker server side (called from ElevatedWorker role).
// Speaks a line protocol on the named pipe:
//   HELLO <token>
//   OP stop_service | start_service | migrate_service <utf8-new-path> |
//      sweep_pfro | ping | quit
// Replies: OK / ERR <msg>
// migrate_service 的新路径参数为行尾全段（UTF-8 无损，路径可含空格/非 ASCII，
// 不作空白切分）。命令间空闲上限 600s（S2 会话态，原单命令 120s）；客户端
// 30s 心跳 ping 维持（见 ElevatedSession），慢盘解压+写入全链期间会话不亡。
int RunElevatedWorkerServer(const std::wstring &pipe_name, const std::wstring &token,
                            const std::wstring &pfro_sweep_dir = L"");

}  // namespace exv::setup
