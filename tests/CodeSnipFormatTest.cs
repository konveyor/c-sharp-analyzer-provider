using Xunit;
using CSharpProvider.Services;
using Provider;
using Microsoft.Extensions.Logging.Abstractions;

namespace CSharpProvider.Tests;

public class CodeSnipFormatTest
{
    [Fact]
    public void GetCodeSnip_Should_Format_With_Line_Numbers()
    {
        // Arrange
        var testFile = Path.GetTempFileName();
        try
        {
            File.WriteAllLines(testFile, new[]
            {
                "line 1",
                "line 2",
                "line 3",
                "line 4",
                "line 5"
            });

            var config = new ProviderConfig("test", 1); // 1 context line
            var service = new CodeLocationService(config, NullLogger<CodeLocationService>.Instance);

            var request = new GetCodeSnipRequest
            {
                Uri = $"file://{testFile}",
                CodeLocation = new Location
                {
                    StartPosition = new Position { Line = 2 }, // line 3 (0-based)
                    EndPosition = new Position { Line = 2 }
                }
            };

            // Act
            var response = service.GetCodeSnip(request, null!).Result;

            // Assert
            var expected = "  2  line 2\n  3  line 3\n  4  line 4";
            Assert.Equal(expected, response.Snip);

            // Verify line number format
            var lines = response.Snip.Split('\n');
            Assert.Equal(3, lines.Length); // 1 context before + target + 1 context after
            Assert.StartsWith("  2  ", lines[0]); // Right-aligned to 3 digits
            Assert.StartsWith("  3  ", lines[1]);
            Assert.StartsWith("  4  ", lines[2]);
        }
        finally
        {
            File.Delete(testFile);
        }
    }

    [Fact]
    public void GetCodeSnip_Should_Format_Large_Line_Numbers()
    {
        // Arrange
        var testFile = Path.GetTempFileName();
        try
        {
            // Create file with 150 lines
            var lines = Enumerable.Range(1, 150).Select(i => $"line {i}").ToArray();
            File.WriteAllLines(testFile, lines);

            var config = new ProviderConfig("test", 2);
            var service = new CodeLocationService(config, NullLogger<CodeLocationService>.Instance);

            var request = new GetCodeSnipRequest
            {
                Uri = $"file://{testFile}",
                CodeLocation = new Location
                {
                    StartPosition = new Position { Line = 99 }, // line 100 (0-based)
                    EndPosition = new Position { Line = 99 }
                }
            };

            // Act
            var response = service.GetCodeSnip(request, null!).Result;

            // Assert - Should handle 3-digit line numbers
            Assert.Contains(" 98  line 98", response.Snip);
            Assert.Contains(" 99  line 99", response.Snip);
            Assert.Contains("100  line 100", response.Snip); // No leading space for 3-digit
            Assert.Contains("101  line 101", response.Snip);
            Assert.Contains("102  line 102", response.Snip);
        }
        finally
        {
            File.Delete(testFile);
        }
    }
}
