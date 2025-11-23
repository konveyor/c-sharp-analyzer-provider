using System;
using System.Collections.Generic;
using System.Linq;

namespace Net8Sample
{
    class Program
    {
        static void Main(string[] args)
        {
            Console.WriteLine("Hello, .NET 8!");

            var numbers = new List<int> { 1, 2, 3, 4, 5 };
            var evenNumbers = numbers.Where(n => n % 2 == 0).ToList();

            foreach (var num in evenNumbers)
            {
                Console.WriteLine($"Even number: {num}");
            }

            var person = new Person("John", 30);
            person.Greet();
        }
    }

    public class Person
    {
        public string Name { get; set; }
        public int Age { get; set; }

        public Person(string name, int age)
        {
            Name = name;
            Age = age;
        }

        public void Greet()
        {
            Console.WriteLine($"Hello, my name is {Name} and I am {Age} years old.");
        }
    }
}
