using System;
using System.Collections.Generic;
using System.Data.Linq;
using System.Linq;
using LegacyApp.Models;
using log4net;

namespace LegacyApp.DataAccess
{
    public class OrderRepository : IDisposable
    {
        private static readonly ILog _log = LogManager.GetLogger(typeof(OrderRepository));
        private readonly DataContext _context;
        private bool _disposed;

        public OrderRepository(string connectionString)
        {
            _context = new DataContext(connectionString);
        }

        public Order GetById(int id)
        {
            Table<Order> orders = _context.GetTable<Order>();
            return orders.SingleOrDefault(o => o.Id == id);
        }

        public List<Order> GetByCustomer(int customerId)
        {
            Table<Order> orders = _context.GetTable<Order>();
            return orders.Where(o => o.CustomerId == customerId)
                         .OrderByDescending(o => o.OrderDate)
                         .ToList();
        }

        public List<Order> GetPendingOrders()
        {
            Table<Order> orders = _context.GetTable<Order>();
            return orders.Where(o => o.Status == OrderStatus.Pending).ToList();
        }

        public decimal GetTotalRevenue()
        {
            Table<Order> orders = _context.GetTable<Order>();
            return orders.Where(o => o.Status != OrderStatus.Cancelled)
                         .Sum(o => o.TotalAmount);
        }

        public void Add(Order order)
        {
            if (order == null)
            {
                throw new ArgumentNullException("order");
            }
            order.OrderDate = DateTime.Now;
            order.Status = OrderStatus.Pending;
            Table<Order> orders = _context.GetTable<Order>();
            orders.InsertOnSubmit(order);
            _context.SubmitChanges();
            _log.InfoFormat("Added order {0} for customer {1}", order.Id, order.CustomerId);
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
