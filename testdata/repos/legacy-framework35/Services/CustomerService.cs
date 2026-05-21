using System;
using System.Collections.Generic;
using System.ServiceModel;
using LegacyApp.DataAccess;
using LegacyApp.Models;
using log4net;

namespace LegacyApp.Services
{
    [ServiceBehavior(InstanceContextMode = InstanceContextMode.PerCall)]
    public class CustomerService : ICustomerService
    {
        private static readonly ILog _log = LogManager.GetLogger(typeof(CustomerService));
        private readonly string _connectionString;

        public CustomerService()
        {
            _connectionString = System.Configuration.ConfigurationManager
                .ConnectionStrings["LegacyDb"].ConnectionString;
        }

        public Customer GetCustomer(int id)
        {
            using (CustomerRepository repo = new CustomerRepository(_connectionString))
            {
                Customer customer = repo.GetById(id);
                if (customer == null)
                {
                    throw new FaultException(
                        string.Format("Customer {0} not found", id));
                }
                return customer;
            }
        }

        public List<Customer> SearchCustomers(string name)
        {
            using (CustomerRepository repo = new CustomerRepository(_connectionString))
            {
                return repo.FindByName(name);
            }
        }

        public void CreateCustomer(Customer customer)
        {
            _log.InfoFormat("Creating customer: {0}", customer.Name);
            using (CustomerRepository repo = new CustomerRepository(_connectionString))
            {
                repo.Add(customer);
            }
        }

        public List<Order> GetCustomerOrders(int customerId)
        {
            using (OrderRepository repo = new OrderRepository(_connectionString))
            {
                return repo.GetByCustomer(customerId);
            }
        }

        public void CancelOrder(int orderId)
        {
            using (OrderRepository repo = new OrderRepository(_connectionString))
            {
                Order order = repo.GetById(orderId);
                if (order == null)
                {
                    throw new FaultException(
                        string.Format("Order {0} not found", orderId));
                }
                order.Cancel();
                _log.InfoFormat("Cancelled order {0}", orderId);
            }
        }
    }
}
