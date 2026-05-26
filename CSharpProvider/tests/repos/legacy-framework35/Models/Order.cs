using System;
using System.Data.Linq.Mapping;

namespace LegacyApp.Models
{
    public enum OrderStatus
    {
        Pending,
        Processing,
        Shipped,
        Delivered,
        Cancelled
    }

    [Table(Name = "Orders")]
    public class Order
    {
        [Column(IsPrimaryKey = true, IsDbGenerated = true)]
        public int Id { get; set; }

        [Column]
        public int CustomerId { get; set; }

        [Column]
        public DateTime OrderDate { get; set; }

        [Column]
        public decimal TotalAmount { get; set; }

        [Column]
        public OrderStatus Status { get; set; }

        [Column]
        public string ShippingAddress { get; set; }

        private EntityRef<Customer> _customer;

        [Association(Storage = "_customer", ThisKey = "CustomerId")]
        public Customer Customer
        {
            get { return _customer.Entity; }
            set { _customer.Entity = value; }
        }

        public bool CanCancel()
        {
            return Status == OrderStatus.Pending || Status == OrderStatus.Processing;
        }

        public void Cancel()
        {
            if (!CanCancel())
            {
                throw new InvalidOperationException(
                    string.Format("Cannot cancel order {0} with status {1}", Id, Status));
            }
            Status = OrderStatus.Cancelled;
        }
    }
}
