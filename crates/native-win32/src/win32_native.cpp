#include <windows.h>
#include <shellapi.h>
#include <shlobj.h>
#include <shobjidl.h>
#include <propkey.h>
#include <winioctl.h>
#include <cstring>
#include <string>
#include <vector>

#if __has_include(<winrt/Microsoft.Windows.AppNotifications.h>)
#define LOCALREF_HAS_APP_NOTIFICATIONS 1
#include <winrt/base.h>
#include <winrt/Microsoft.Windows.AppNotifications.h>
#else
#define LOCALREF_HAS_APP_NOTIFICATIONS 0
#endif

#if __has_include(<winrt/Windows.UI.Notifications.h>) && __has_include(<winrt/Windows.Data.Xml.Dom.h>)
#define LOCALREF_HAS_CLASSIC_TOASTS 1
#include <winrt/base.h>
#include <winrt/Windows.Data.Xml.Dom.h>
#include <winrt/Windows.UI.Notifications.h>
#else
#define LOCALREF_HAS_CLASSIC_TOASTS 0
#endif

namespace {

constexpr DWORD kUnsupported = 50;
constexpr wchar_t kAppUserModelId[] = L"Localref.Desktop";
constexpr wchar_t kShortcutName[] = L"Localref.lnk";

typedef struct {
    ULONG ReparseTag;
    USHORT ReparseDataLength;
    USHORT Reserved;
    struct {
        USHORT SubstituteNameOffset;
        USHORT SubstituteNameLength;
        USHORT PrintNameOffset;
        USHORT PrintNameLength;
        WCHAR PathBuffer[1];
    } MountPointReparseBuffer;
} LocalrefReparseDataBuffer;

std::wstring utf8_to_wide(const char* value) {
    if (value == nullptr) {
        return {};
    }
    const int needed = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value, -1, nullptr, 0);
    if (needed <= 0) {
        return {};
    }
    std::wstring wide(static_cast<size_t>(needed - 1), L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value, -1, wide.data(), needed);
    return wide;
}

DWORD shell_open(const char* value) {
    const std::wstring wide = utf8_to_wide(value);
    if (wide.empty()) {
        return ERROR_INVALID_PARAMETER;
    }
    HINSTANCE result = ShellExecuteW(
        nullptr,
        L"open",
        wide.c_str(),
        nullptr,
        nullptr,
        SW_SHOWNORMAL);
    const auto code = reinterpret_cast<INT_PTR>(result);
    if (code <= 32) {
        return static_cast<DWORD>(code);
    }
    return ERROR_SUCCESS;
}

DWORD hresult_code(HRESULT result) {
    return SUCCEEDED(result) ? ERROR_SUCCESS : static_cast<DWORD>(result);
}

DWORD initialize_com() {
    const HRESULT result = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    if (SUCCEEDED(result) || result == RPC_E_CHANGED_MODE) {
        return ERROR_SUCCESS;
    }
    return hresult_code(result);
}

std::wstring current_executable_path() {
    std::vector<wchar_t> buffer(MAX_PATH);
    for (;;) {
        const DWORD length = GetModuleFileNameW(
            nullptr,
            buffer.data(),
            static_cast<DWORD>(buffer.size()));
        if (length == 0) {
            return {};
        }
        if (length < buffer.size() - 1) {
            return std::wstring(buffer.data(), length);
        }
        buffer.resize(buffer.size() * 2);
    }
}

std::wstring start_menu_shortcut_path() {
    PWSTR programs_path = nullptr;
    const HRESULT result = SHGetKnownFolderPath(
        FOLDERID_Programs,
        KF_FLAG_CREATE,
        nullptr,
        &programs_path);
    if (FAILED(result)) {
        return {};
    }
    std::wstring path(programs_path);
    CoTaskMemFree(programs_path);
    if (!path.empty() && path.back() != L'\\') {
        path.push_back(L'\\');
    }
    path += kShortcutName;
    return path;
}

bool shortcut_exists() {
    const std::wstring shortcut = start_menu_shortcut_path();
    return !shortcut.empty() && GetFileAttributesW(shortcut.c_str()) != INVALID_FILE_ATTRIBUTES;
}

DWORD ensure_app_user_model_shortcut(const char* icon_value) {
#if LOCALREF_HAS_CLASSIC_TOASTS
    const DWORD initialized = initialize_com();
    if (initialized != ERROR_SUCCESS) {
        return initialized;
    }

    const std::wstring executable = current_executable_path();
    const std::wstring shortcut = start_menu_shortcut_path();
    const std::wstring icon = utf8_to_wide(icon_value);
    if (executable.empty() || shortcut.empty()) {
        return ERROR_PATH_NOT_FOUND;
    }

    try {
        winrt::com_ptr<IShellLinkW> link;
        HRESULT result = CoCreateInstance(
            CLSID_ShellLink,
            nullptr,
            CLSCTX_INPROC_SERVER,
            IID_PPV_ARGS(link.put()));
        if (FAILED(result)) {
            return hresult_code(result);
        }
        result = link->SetPath(executable.c_str());
        if (FAILED(result)) {
            return hresult_code(result);
        }
        result = link->SetArguments(L"");
        if (FAILED(result)) {
            return hresult_code(result);
        }
        if (!icon.empty()) {
            result = link->SetIconLocation(icon.c_str(), 0);
            if (FAILED(result)) {
                return hresult_code(result);
            }
        }

        winrt::com_ptr<IPropertyStore> property_store;
        result = link.as(IID_PPV_ARGS(property_store.put()));
        if (FAILED(result)) {
            return hresult_code(result);
        }
        PROPVARIANT app_id = {};
        app_id.vt = VT_LPWSTR;
        app_id.pwszVal = const_cast<PWSTR>(kAppUserModelId);
        result = property_store->SetValue(PKEY_AppUserModel_ID, app_id);
        if (FAILED(result)) {
            return hresult_code(result);
        }
        result = property_store->Commit();
        if (FAILED(result)) {
            return hresult_code(result);
        }

        winrt::com_ptr<IPersistFile> file;
        result = link.as(IID_PPV_ARGS(file.put()));
        if (FAILED(result)) {
            return hresult_code(result);
        }
        return hresult_code(file->Save(shortcut.c_str(), TRUE));
    } catch (...) {
        return GetLastError() == ERROR_SUCCESS ? ERROR_GEN_FAILURE : GetLastError();
    }
#else
    return kUnsupported;
#endif
}

std::wstring nt_substitute_name(const std::wstring& path) {
    if (path.rfind(L"\\\\?\\", 0) == 0) {
        return L"\\??\\" + path.substr(4);
    }
    return L"\\??\\" + path;
}

DWORD create_mount_point(const char* link_value, const char* target_value) {
    const std::wstring link = utf8_to_wide(link_value);
    const std::wstring target = utf8_to_wide(target_value);
    if (link.empty() || target.empty()) {
        return ERROR_INVALID_PARAMETER;
    }
    if (!CreateDirectoryW(link.c_str(), nullptr) && GetLastError() != ERROR_ALREADY_EXISTS) {
        return GetLastError();
    }

    HANDLE handle = CreateFileW(
        link.c_str(),
        GENERIC_WRITE,
        0,
        nullptr,
        OPEN_EXISTING,
        FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
        nullptr);
    if (handle == INVALID_HANDLE_VALUE) {
        return GetLastError();
    }

    const std::wstring substitute = nt_substitute_name(target);
    const std::wstring print = target;
    const USHORT substitute_bytes = static_cast<USHORT>(substitute.size() * sizeof(wchar_t));
    const USHORT print_bytes = static_cast<USHORT>(print.size() * sizeof(wchar_t));
    const USHORT path_bytes = substitute_bytes + sizeof(wchar_t) + print_bytes + sizeof(wchar_t);
    const DWORD buffer_bytes = FIELD_OFFSET(LocalrefReparseDataBuffer, MountPointReparseBuffer.PathBuffer) + path_bytes;
    std::vector<BYTE> buffer(buffer_bytes);
    auto* reparse = reinterpret_cast<LocalrefReparseDataBuffer*>(buffer.data());
    reparse->ReparseTag = IO_REPARSE_TAG_MOUNT_POINT;
    reparse->ReparseDataLength = static_cast<USHORT>(
        path_bytes + 4 * sizeof(USHORT));
    reparse->Reserved = 0;
    reparse->MountPointReparseBuffer.SubstituteNameOffset = 0;
    reparse->MountPointReparseBuffer.SubstituteNameLength = substitute_bytes;
    reparse->MountPointReparseBuffer.PrintNameOffset = substitute_bytes + sizeof(wchar_t);
    reparse->MountPointReparseBuffer.PrintNameLength = print_bytes;

    wchar_t* path_buffer = reparse->MountPointReparseBuffer.PathBuffer;
    memcpy(path_buffer, substitute.data(), substitute_bytes);
    path_buffer[substitute.size()] = L'\0';
    memcpy(path_buffer + substitute.size() + 1, print.data(), print_bytes);
    path_buffer[substitute.size() + 1 + print.size()] = L'\0';

    DWORD returned = 0;
    const BOOL ok = DeviceIoControl(
        handle,
        FSCTL_SET_REPARSE_POINT,
        reparse,
        buffer_bytes,
        nullptr,
        0,
        &returned,
        nullptr);
    const DWORD error = ok ? ERROR_SUCCESS : GetLastError();
    CloseHandle(handle);
    return error;
}

std::wstring xml_escape(const std::wstring& value);

std::wstring toast_xml(const char* title_value, const char* body_value, int kind) {
    const std::wstring title = xml_escape(utf8_to_wide(title_value));
    const std::wstring body = xml_escape(utf8_to_wide(body_value));
    const wchar_t* label = kind == 2 ? L"Error" : (kind == 1 ? L"Success" : L"Info");
    return L"<toast><visual><binding template=\"ToastGeneric\"><text>Localref "
        + std::wstring(label)
        + L"</text><text>"
        + title
        + L"</text><text>"
        + body
        + L"</text></binding></visual></toast>";
}

DWORD show_classic_toast(const char* title, const char* body, int kind) {
#if LOCALREF_HAS_CLASSIC_TOASTS
    if (!shortcut_exists()) {
        const DWORD registered = ensure_app_user_model_shortcut(nullptr);
        if (registered != ERROR_SUCCESS) {
            return registered;
        }
    }
    try {
        winrt::init_apartment(winrt::apartment_type::single_threaded);
        winrt::Windows::Data::Xml::Dom::XmlDocument document;
        document.LoadXml(winrt::hstring(toast_xml(title, body, kind)));
        winrt::Windows::UI::Notifications::ToastNotification notification(document);
        winrt::Windows::UI::Notifications::ToastNotificationManager
            ::CreateToastNotifier(kAppUserModelId)
            .Show(notification);
        return ERROR_SUCCESS;
    } catch (...) {
        return GetLastError() == ERROR_SUCCESS ? ERROR_GEN_FAILURE : GetLastError();
    }
#else
    return kUnsupported;
#endif
}

std::wstring xml_escape(const std::wstring& value) {
    std::wstring escaped;
    for (wchar_t ch : value) {
        switch (ch) {
        case L'&':
            escaped += L"&amp;";
            break;
        case L'<':
            escaped += L"&lt;";
            break;
        case L'>':
            escaped += L"&gt;";
            break;
        case L'"':
            escaped += L"&quot;";
            break;
        case L'\'':
            escaped += L"&apos;";
            break;
        default:
            escaped.push_back(ch);
            break;
        }
    }
    return escaped;
}

}

extern "C" DWORD native_open_path(const char* path) {
    return shell_open(path);
}

extern "C" DWORD native_open_uri(const char* uri) {
    return shell_open(uri);
}

extern "C" DWORD native_create_directory_junction(const char* link, const char* target) {
    return create_mount_point(link, target);
}

extern "C" DWORD native_register_app_notifications() {
#if LOCALREF_HAS_APP_NOTIFICATIONS
    try {
        winrt::init_apartment();
        winrt::Microsoft::Windows::AppNotifications::AppNotificationManager::Default().Register();
        return ERROR_SUCCESS;
    } catch (...) {
        return GetLastError() == ERROR_SUCCESS ? ERROR_GEN_FAILURE : GetLastError();
    }
#elif LOCALREF_HAS_CLASSIC_TOASTS
    return ensure_app_user_model_shortcut(nullptr);
#else
    return kUnsupported;
#endif
}

extern "C" DWORD native_register_app_notifications_with_icon(const char* icon_path) {
#if LOCALREF_HAS_APP_NOTIFICATIONS
    return native_register_app_notifications();
#elif LOCALREF_HAS_CLASSIC_TOASTS
    return ensure_app_user_model_shortcut(icon_path);
#else
    return kUnsupported;
#endif
}

extern "C" DWORD native_unregister_app_notifications() {
#if LOCALREF_HAS_APP_NOTIFICATIONS
    try {
        winrt::Microsoft::Windows::AppNotifications::AppNotificationManager::Default().Unregister();
        return ERROR_SUCCESS;
    } catch (...) {
        return GetLastError() == ERROR_SUCCESS ? ERROR_GEN_FAILURE : GetLastError();
    }
#else
    return ERROR_SUCCESS;
#endif
}

extern "C" DWORD native_show_app_notification(const char* title, const char* body, int kind) {
#if LOCALREF_HAS_APP_NOTIFICATIONS
    try {
        winrt::init_apartment();
        const std::wstring xml = toast_xml(title, body, kind);
        winrt::Microsoft::Windows::AppNotifications::AppNotification notification(xml);
        winrt::Microsoft::Windows::AppNotifications::AppNotificationManager::Default().Show(notification);
        return ERROR_SUCCESS;
    } catch (...) {
        return GetLastError() == ERROR_SUCCESS ? ERROR_GEN_FAILURE : GetLastError();
    }
#elif LOCALREF_HAS_CLASSIC_TOASTS
    return show_classic_toast(title, body, kind);
#else
    return kUnsupported;
#endif
}

extern "C" int native_app_notifications_available() {
    return LOCALREF_HAS_APP_NOTIFICATIONS || LOCALREF_HAS_CLASSIC_TOASTS;
}

extern "C" DWORD native_detach_console() {
    FreeConsole();
    return ERROR_SUCCESS;
}
