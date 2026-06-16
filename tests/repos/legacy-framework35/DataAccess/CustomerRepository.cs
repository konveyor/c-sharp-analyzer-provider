using System;
using System.Collections.Generic;
using System.Data.Linq;
using System.Linq;
using LegacyApp.Models;
using log4net;

namespace LegacyApp.DataAccess
{
    public class CustomerRepository : IDisposable
    {
        private static readonly ILog _log = LogManager.GetLogger(typeof(CustomerRepository));
        private readonly DataContext _context;
        private bool _disposed;

        public CustomerRepository(string connectionString)
        {
            _context = new DataContext(connectionString);
            _log.Info("CustomerRepository initialized");
        }

        public Customer GetById(int id)
        {
            _log.DebugFormat("Looking up customer {0}", id);
            Table<Customer> customers = _context.GetTable<Customer>();
            Customer customer = customers.SingleOrDefault(c => c.Id == id);
            if (customer == null)
            {
                _log.WarnFormat("Customer {0} not found", id);
            }
            return customer;
        }

        public List<Customer> GetAll()
        {
            Table<Customer> customers = _context.GetTable<Customer>();
            return customers.ToList();
        }

        public List<Customer> FindByName(string name)
        {
            Table<Customer> customers = _context.GetTable<Customer>();
            return customers.Where(c => c.Name.Contains(name)).ToList();
        }

        public void Add(Customer customer)
        {
            if (customer == null)
            {
                throw new ArgumentNullException("customer");
            }
            customer.CreatedDate = DateTime.Now;
            Table<Customer> customers = _context.GetTable<Customer>();
            customers.InsertOnSubmit(customer);
            _context.SubmitChanges();
            _log.InfoFormat("Added customer {0}: {1}", customer.Id, customer.Name);
        }

        public void Delete(Customer customer)
        {
            Table<Customer> customers = _context.GetTable<Customer>();
            customers.DeleteOnSubmit(customer);
            _context.SubmitChanges();
            _log.InfoFormat("Deleted customer {0}", customer.Id);
        }

        public void Dispose()
        {
            if (!_disposed)
            {
                _context.Dispose();
                _disposed = true;
            }
        }
    }
}
