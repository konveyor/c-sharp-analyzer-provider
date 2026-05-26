using System;
using System.Collections.Generic;
using System.Data.Linq.Mapping;

namespace LegacyApp.Models
{
    [Table(Name = "Customers")]
    public class Customer
    {
        [Column(IsPrimaryKey = true, IsDbGenerated = true)]
        public int Id { get; set; }

        [Column]
        public string Name { get; set; }

        [Column]
        public string Email { get; set; }

        [Column]
        public DateTime CreatedDate { get; set; }

        [Column]
        public bool IsActive { get; set; }

        private EntitySet<Order> _orders = new EntitySet<Order>();

        [Association(Storage = "_orders", OtherKey = "CustomerId")]
        public EntitySet<Order> Orders
        {
            get { return _orders; }
            set { _orders.Assign(value); }
        }

        public string GetDisplayName()
        {
            return string.Format("{0} ({1})", Name, Email);
        }

        public List<Order> GetActiveOrders()
        {
            List<Order> active = new List<Order>();
            foreach (Order order in Orders)
            {
                if (order.Status != OrderStatus.Cancelled)
                {
                    active.Add(order);
                }
            }
            return active;
        }
    }
}
