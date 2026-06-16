using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

var obj = new { Name = "test", Value = 42 };
string json = JsonConvert.SerializeObject(obj, Formatting.Indented);
Console.WriteLine(json);

var parsed = JObject.Parse(json);
string? name = parsed["Name"]?.Value<string>();
Console.WriteLine($"Name: {name}");

var settings = new JsonSerializerSettings
{
    NullValueHandling = NullValueHandling.Ignore,
    Formatting = Formatting.Indented,
};
string json2 = JsonConvert.SerializeObject(obj, settings);
Console.WriteLine(json2);
