using System.Runtime.Versioning;
using Velopack.Windows;

namespace Velopack.Tests;

[SupportedOSPlatform("windows")]
public class KnownPathsTests
{
    [Fact]
    public void DefaultRootIsUnderPrograms()
    {
        Assert.SkipUnless(VelopackRuntimeInfo.IsWindows, "Windows only");

        var root = KnownPaths.GetAppRootDir("CoolApp");
        var localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);

        Assert.Equal(Path.Combine(localAppData, "Programs", "CoolApp"), root);
    }

    /// <summary>
    /// An uninstaller walks both locations to clean up a left-over packages cache, so neither may
    /// contain the other - otherwise removing one would remove somebody else's install.
    /// </summary>
    [Fact]
    public void PreviousRootIsASiblingNotAnAncestor()
    {
        Assert.SkipUnless(VelopackRuntimeInfo.IsWindows, "Windows only");

        var root = KnownPaths.GetAppRootDir("CoolApp");
        var previous = KnownPaths.GetLegacyAppRootDir("CoolApp");

        Assert.NotEqual(root, previous);
        Assert.False(root!.StartsWith(previous!, StringComparison.OrdinalIgnoreCase));
        Assert.False(previous!.StartsWith(root!, StringComparison.OrdinalIgnoreCase));
    }

    [Fact]
    public void AnUnknownAppHasNoRoot()
    {
        Assert.SkipUnless(VelopackRuntimeInfo.IsWindows, "Windows only");

        Assert.Null(KnownPaths.GetAppRootDir(null));
        Assert.Null(KnownPaths.GetAppRootDir(""));
        Assert.Null(KnownPaths.GetLegacyAppRootDir(""));
    }
}
