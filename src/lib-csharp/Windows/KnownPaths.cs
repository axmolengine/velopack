using System;
using System.IO;
using System.Runtime.Versioning;

namespace Velopack.Windows
{
    /// <summary>
    /// Resolves where an application's per-user files live. The default install root is also where
    /// an install with a read-only root (a Program Files MSI, for example) keeps its packages,
    /// because it cannot write into itself.
    /// </summary>
    /// <remarks>
    /// This is the second statement of that location; the first is
    /// src/lib-rust/src/known_path.rs::default_app_root_dir and the third is the value
    /// MsiTemplateData gives Windows Installer for a per-user install. All three have to agree.
    /// </remarks>
    [SupportedOSPlatform("windows")]
    internal static class KnownPaths
    {
        /// <summary>
        /// The directory under LocalAppData that per-user applications are installed into. There is
        /// no known folder for it (the shell's "Programs" folder is the Start Menu's), so the name
        /// is joined literally. It is shared with other publishers' applications and is never
        /// deleted on behalf of any single one of them.
        /// </summary>
        const string ProgramsFolderName = "Programs";

        /// <summary>
        /// Where a per-user install of the given application goes by default.
        /// </summary>
        public static string? GetAppRootDir(string? appId)
        {
            var localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            if (string.IsNullOrEmpty(localAppData) || string.IsNullOrEmpty(appId)) return null;
            return Path.Combine(localAppData, ProgramsFolderName, appId);
        }

        /// <summary>
        /// Where a per-user install went before applications moved under the Programs directory.
        /// An install found here is upgraded in place rather than duplicated, and a packages cache
        /// left here can be cleaned up.
        /// </summary>
        public static string? GetLegacyAppRootDir(string? appId)
        {
            var localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            if (string.IsNullOrEmpty(localAppData) || string.IsNullOrEmpty(appId)) return null;
            return Path.Combine(localAppData, appId);
        }
    }
}
