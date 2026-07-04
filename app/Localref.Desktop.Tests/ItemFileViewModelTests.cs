using Localref.Desktop.ViewModels;
using uniffi.localref_ffi;

namespace Localref.Desktop.Tests;

public class ItemFileViewModelTests
{
    [Fact]
    public void MainFile_IsDetectedAndLabeled()
    {
        var entry = new ItemFileEntry("paper.pdf", "file", 1_572_864);

        var file = new ItemFileViewModel(entry, "PAPER.PDF");

        Assert.True(file.IsMain);
        Assert.True(file.CanBeMain);
        Assert.Equal("MAIN FILE", file.Role);
        Assert.Equal("1.5 MB", file.Size);
    }

    [Fact]
    public void MetadataFile_CannotBecomeMain()
    {
        var entry = new ItemFileEntry("metadata.toml", "file", 4200);

        var file = new ItemFileViewModel(entry, null);

        Assert.False(file.IsMain);
        Assert.False(file.CanBeMain);
        Assert.Equal("METADATA", file.Role);
    }
}
